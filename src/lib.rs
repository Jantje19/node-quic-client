use std::{net::SocketAddr, sync::Arc};

use cancel_with_value::CancelWithValue;
use neon::{prelude::*, types::JsBigInt};
use once_cell::sync::OnceCell;
use quinn::{ClosedStream, ConnectionError, RecvStream, SendStream, StreamId, VarInt, WriteError};
use take_once::TakeOnce;
use tokio::{runtime::Runtime, sync::Mutex, task::JoinHandle};

mod cancel_with_value;
mod quic;
mod take_once;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();

// Return a global tokio runtime or create one if it doesn't exist.
// Throws a JavaScript exception if the `Runtime` fails to create.
fn runtime<'a, C: Context<'a>>(cx: &mut C) -> NeonResult<&'static Runtime> {
    RUNTIME.get_or_try_init(|| Runtime::new().or_else(|err| cx.throw_error(err.to_string())))
}

#[derive(Clone)]
struct Connection {
    connection: Arc<quinn::Connection>,
    close_handle: Arc<JoinHandle<()>>,
    stream_handle: Arc<JoinHandle<()>>,
}

impl Finalize for Connection {
    fn finalize<'a, C: Context<'a>>(self, _: &mut C) {
        self.close_handle.abort();
        self.stream_handle.abort();
        self.connection.close(0u8.into(), b"");
    }
}

fn to_uint8_vec<'a, C>(
    cx: &mut C,
    value: Option<Handle<JsArray>>,
) -> Result<Option<Vec<Vec<u8>>>, neon::result::Throw>
where
    C: Context<'a>,
{
    Ok(match value {
        None => None,
        Some(protocols) => {
            let value: Result<Vec<_>, neon::result::Throw> = protocols
                .to_vec(cx)?
                .into_iter()
                .map(|entry| {
                    use neon::types::buffer::TypedArray;

                    entry
                        .downcast_or_throw(cx)
                        .map(|handle: Handle<JsUint8Array>| handle.as_slice(cx).to_vec())
                })
                .collect();

            Some(value?)
        }
    })
}

fn connect(mut cx: FunctionContext) -> JsResult<JsPromise> {
    let port = cx.argument::<JsNumber>(0)?.value(&mut cx) as u16;
    let ip = cx.argument::<JsString>(1)?.value(&mut cx);
    let hostname = cx.argument::<JsString>(2)?.value(&mut cx);
    let on_stream = cx.argument::<JsFunction>(3)?.root(&mut cx);
    let on_close = cx.argument::<JsFunction>(4)?.root(&mut cx);
    let on_error = cx.argument::<JsFunction>(5)?.root(&mut cx);
    let alpn_protocols: Option<Handle<JsArray>> = cx.argument::<JsValue>(6)?.downcast(&mut cx).ok();
    let certificate_authorities: Option<Handle<JsArray>> =
        cx.argument::<JsValue>(7)?.downcast(&mut cx).ok();

    let client_auth = {
        let args: Option<Handle<JsArray>> = cx.argument::<JsValue>(6)?.downcast(&mut cx).ok();

        to_uint8_vec(&mut cx, args)?.and_then(|args| {
            if args.len() < 2 {
                return None;
            }

            let mut args = args.into_iter();
            let cert = args.next().unwrap();
            let key = args.next().unwrap();

            Some((cert, key))
        })
    };

    let alpn_protocols = to_uint8_vec(&mut cx, alpn_protocols)?;
    let certificate_authorities = to_uint8_vec(&mut cx, certificate_authorities)?;

    let addr = SocketAddr::new(ip.parse().unwrap(), port);
    let rt = runtime(&mut cx)?;

    let on_stream_channel = cx.channel();
    let on_close_channel = cx.channel();
    let on_error_channel = cx.channel();

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        let result = quic::get_client(
            addr,
            &hostname,
            alpn_protocols,
            certificate_authorities,
            client_auth,
        )
        .await;

        deferred.settle_with(&channel, move |mut cx| {
            let (connection, endpoint) = result.or_else(|err| cx.throw_error(err.to_string()))?;
            let connection = Arc::new(connection);
            let endpoint = Arc::new(endpoint);

            let close_handle = {
                let connection = connection.clone();
                let endpoint = endpoint.clone();
                rt.spawn(async move {
                    let reason = connection.closed().await;
                    endpoint.wait_idle().await;

                    on_close_channel.send(move |mut cx| {
                        let callback = on_close.into_inner(&mut cx);
                        let this = cx.undefined();

                        let args = vec![cx.string(reason.to_string()).upcast()];

                        callback.call(&mut cx, this, args)?;

                        Ok(())
                    });
                })
            };

            let stream_handle = {
                let connection = connection.clone();
                let on_error = Arc::new(on_error);
                let on_stream = Arc::new(on_stream);

                rt.spawn(async move {
                    fn handle_bidi<E, S>(
                        result: Result<(SendStream, RecvStream), ConnectionError>,
                        error_handler: E,
                        stream_handler: S,
                    ) -> bool where
                        E: FnOnce(ConnectionError) -> bool,
                        S: FnOnce(PartialStream)
                    {
                        let (send, recv) = match result {
                            Err(err) => return error_handler(err),
                            Ok(v) => v,
                        };

                        let stream = PartialStream {
                            send: Arc::new(TakeOnce::new(Some(send))),
                            recv: Arc::new(TakeOnce::new(recv)),
                        };

                        stream_handler(stream);

                        false
                    }

                    fn handle_uni<E, S>(
                        result: Result<RecvStream, ConnectionError>,
                        error_handler: E,
                        stream_handler: S,
                    )  -> bool where
                        E: FnOnce(ConnectionError) -> bool,
                        S: FnOnce(PartialStream)
                    {
                        let recv = match result {
                            Err(err) => return error_handler(err),
                            Ok(v) => v,
                        };

                        let stream = PartialStream {
                            send: Arc::new(TakeOnce::new(None)),
                            recv: Arc::new(TakeOnce::new(recv)),
                        };

                        stream_handler(stream);

                        false
                    }

                    loop {
                        let on_error_channel = on_error_channel.clone();
                        let on_error = on_error.clone();
                        let handle_error = |error: ConnectionError| {
                            match  error {
                                ConnectionError::ConnectionClosed(_) |
                                ConnectionError::ApplicationClosed(_) |
                                ConnectionError::Reset |
                                ConnectionError::LocallyClosed  => {},
                                _ => {
                                    on_error_channel.send(move |mut cx| {
                                        let callback = on_error.as_ref().clone(&mut cx).into_inner(&mut cx);
                                        let this = cx.undefined();

                                        let args = &[cx.error(error.to_string()).unwrap().upcast()];

                                        callback.call(&mut cx, this, args)?;

                                        Ok(())
                                    });
                                }
                            }

                            true
                        };

                        let handle_stream = |stream: PartialStream| {
                            let on_stream_channel = on_stream_channel.clone();
                            let on_stream = on_stream.clone();
                            rt.spawn(async move {
                                on_stream_channel.send(move |mut cx| {
                                    let callback = on_stream.as_ref().clone(&mut cx).into_inner(&mut cx);
                                    let this = cx.undefined();

                                    let is_uni = stream.send.peek(|v| v.is_none());

                                    let args: &[Handle<JsValue>] = &[cx.boxed(stream).upcast(), cx.boolean(is_uni).upcast()];

                                    callback.call(&mut cx, this, args)?;

                                    Ok(())
                                });
                            });
                        };

                        tokio::select! {
                            stream = connection.accept_bi() => if handle_bidi(stream, handle_error, handle_stream) { break; },
                            stream = connection.accept_uni() => if handle_uni(stream, handle_error, handle_stream) { break; },
                        }
                    }
                })
            };

            Ok(cx.boxed(Connection {
                connection,
                close_handle: Arc::new(close_handle),
                stream_handle: Arc::new(stream_handle),
            }))
        });
    });

    Ok(promise)
}

struct PartialStream {
    send: Arc<TakeOnce<Option<SendStream>>>,
    recv: Arc<TakeOnce<RecvStream>>,
}

impl Finalize for PartialStream {
    // Do nothing since `initialize_stream` must be called immediately after
    fn finalize<'a, C: Context<'a>>(self, _: &mut C) {}
}

#[derive(Clone, Debug)]
struct StreamDetails {
    id: StreamId,
    is_0rtt: bool,
}

impl StreamDetails {
    fn new(recv: &RecvStream) -> Self {
        Self {
            id: recv.id(),
            is_0rtt: recv.is_0rtt(),
        }
    }
}

#[derive(Clone)]
struct Stream {
    send: Arc<Option<Mutex<SendStream>>>,
    handle: Arc<JoinHandle<()>>,
    details: StreamDetails,
    close_requested: CancelWithValue<VarInt>,
}

impl Finalize for Stream {
    fn finalize<'a, C: Context<'a>>(self, _: &mut C) {
        let rt = RUNTIME.get().unwrap();

        self.handle.clone().abort();

        rt.spawn(async move {
            if let Some(send) = self.send.clone().as_ref() {
                let _ = send.lock().await.finish();
            }
        });
    }
}

async fn handle_read(
    mut recv: quinn::RecvStream,
    close_requested: CancelWithValue<VarInt>,
    data: (Root<JsFunction>, Channel),
    close: (Root<JsFunction>, Channel),
    error: (Root<JsFunction>, Channel),
) {
    let mut buf = [0u8; 2048];

    let data_callback = Arc::new(data.0);
    let close_callback = Arc::new(close.0);
    let error_callback = Arc::new(error.0);

    let handle_close = |reason: String| {
        let callback = close_callback.clone();
        close.1.send(move |mut cx| {
            let callback = callback.as_ref().clone(&mut cx).into_inner(&mut cx);
            let this = cx.undefined();

            let args = vec![cx.string(reason).upcast()];

            callback.call(&mut cx, this, args)?;

            Ok(())
        });
    };

    loop {
        let read_result = tokio::select! {
            result = recv.read(&mut buf) => result,
            error_code = close_requested.cancelled() => {
                let _ = recv.stop(error_code);

                break;
            },
        };

        match read_result {
            Err(e) => match e {
                quinn::ReadError::ConnectionLost(e) => {
                    handle_close(e.to_string());
                    return;
                }
                quinn::ReadError::ClosedStream | quinn::ReadError::Reset(_) => {
                    handle_close(e.to_string());
                    return;
                }
                quinn::ReadError::IllegalOrderedRead | quinn::ReadError::ZeroRttRejected => {
                    let callback = error_callback.clone();
                    error.1.send(move |mut cx| {
                        let callback = callback.as_ref().clone(&mut cx).into_inner(&mut cx);
                        let this = cx.undefined();

                        let args = &[cx.error(e.to_string()).unwrap().upcast()];

                        callback.call(&mut cx, this, args)?;

                        Ok(())
                    });
                }
            },
            Ok(option) => match option {
                None => break,
                Some(n) => {
                    let packet = buf[..n].to_vec();

                    let callback = data_callback.clone();
                    data.1.send(move |mut cx| {
                        let callback = callback.as_ref().clone(&mut cx).into_inner(&mut cx);
                        let this = cx.undefined();

                        let array = {
                            let a = JsUint8Array::new(&mut cx, packet.len())?;
                            for (i, n) in packet.iter().enumerate() {
                                let v = cx.number(*n);
                                a.set(&mut cx, i as u32, v)?;
                            }
                            a
                        };

                        let args = vec![array.upcast()];

                        callback.call(&mut cx, this, args)?;

                        Ok(())
                    });
                }
            },
        }
    }

    handle_close(String::from("closed"));
}

fn create_stream(mut cx: FunctionContext) -> JsResult<JsPromise> {
    let connection = (**cx.argument::<JsBox<Connection>>(0)?).clone();

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        let result = connection.connection.open_bi().await;

        deferred.settle_with(&channel, move |mut cx| {
            let (send, recv) = result.or_else(|err| cx.throw_error(err.to_string()))?;

            let partial_stream = PartialStream {
                send: Arc::new(TakeOnce::new(Some(send))),
                recv: Arc::new(TakeOnce::new(recv)),
            };

            Ok(cx.boxed(partial_stream))
        });
    });

    Ok(promise)
}

fn initialize_stream(mut cx: FunctionContext) -> JsResult<JsBox<Stream>> {
    let partial_stream = cx.argument::<JsBox<PartialStream>>(0)?;
    let on_data = cx.argument::<JsFunction>(1)?.root(&mut cx);
    let on_close = cx.argument::<JsFunction>(2)?.root(&mut cx);
    let on_error = cx.argument::<JsFunction>(3)?.root(&mut cx);

    let rt = runtime(&mut cx)?;

    let data_channel = cx.channel();
    let close_channel = cx.channel();
    let error_channel = cx.channel();

    let send = partial_stream.send.clone().take();
    let recv = partial_stream.recv.clone().take();

    let details = StreamDetails::new(&recv);
    let close_requested = CancelWithValue::new();

    let handle = rt.spawn({
        let close_requested = close_requested.clone();

        async move {
            handle_read(
                recv,
                close_requested,
                (on_data, data_channel),
                (on_close, close_channel),
                (on_error, error_channel),
            )
            .await
        }
    });

    let stream = Stream {
        send: Arc::new(send.map(Mutex::new)),
        handle: Arc::new(handle),
        close_requested,
        details,
    };

    Ok(cx.boxed(stream))
}

fn write_stream(mut cx: FunctionContext) -> JsResult<JsPromise> {
    use neon::types::buffer::TypedArray;

    let stream = (**cx.argument::<JsBox<Stream>>(0)?).clone();
    let packet = cx.argument::<JsTypedArray<u8>>(1)?.as_slice(&cx).to_vec();

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        let result = {
            match stream
                .send
                .clone()
                .as_ref()
                .as_ref()
                .ok_or(WriteError::ClosedStream)
            {
                Err(e) => Err(e),
                Ok(send) => {
                    let mut send = send.lock().await;

                    send.write_all(&packet).await
                }
            }
        };

        deferred.settle_with(&channel, move |mut cx| {
            result.or_else(|err| cx.throw_error(err.to_string()))?;

            Ok(cx.undefined())
        });
    });

    Ok(promise)
}

fn close_stream(mut cx: FunctionContext) -> JsResult<JsPromise> {
    let stream = (**cx.argument::<JsBox<Stream>>(0)?).clone();

    let error_code = {
        let arg = cx.argument::<JsNumber>(1)?;
        let value = arg.value(&mut cx) as u64;

        VarInt::from_u64(value).or_else(|e| cx.throw_error(e.to_string()))?
    };

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        if let Some(send) = stream.send.clone().as_ref() {
            let mut send = send.lock().await;

            // Returns an error if the request gets closed multiple times, but we allow that to happen
            //  So we can just ignore it
            let _ = send.finish();
        }

        stream.close_requested.cancel(error_code);

        deferred.settle_with(&channel, move |mut cx| Ok(cx.undefined()));
    });

    Ok(promise)
}

fn close_write(mut cx: FunctionContext) -> JsResult<JsPromise> {
    let stream = (**cx.argument::<JsBox<Stream>>(0)?).clone();

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        let result = match stream
            .send
            .clone()
            .as_ref()
            .as_ref()
            .ok_or(ClosedStream::new())
        {
            Err(e) => Err(e),
            Ok(send) => {
                let mut send = send.lock().await;

                send.finish()
            }
        };

        deferred.settle_with(&channel, move |mut cx| {
            result.or_else(|err| cx.throw_error(err.to_string()))?;

            Ok(cx.undefined())
        });
    });

    Ok(promise)
}

fn close_connection(mut cx: FunctionContext) -> JsResult<JsPromise> {
    let connection = (**cx.argument::<JsBox<Connection>>(0)?).clone();
    let code = cx.argument::<JsNumber>(1)?.value(&mut cx);
    let reason = {
        let arg = cx.argument::<JsValue>(2)?;

        if arg.is_a::<JsUint8Array, _>(&mut cx) {
            use neon::types::buffer::TypedArray;

            let arr: Handle<JsUint8Array> = arg.downcast_or_throw(&mut cx)?;

            arr.as_slice(&cx).to_vec()
        } else {
            Vec::new()
        }
    };

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        connection.connection.close((code as u32).into(), &reason);

        deferred.settle_with(&channel, move |mut cx| Ok(cx.undefined()));
    });

    Ok(promise)
}

fn stream_details(mut cx: FunctionContext) -> JsResult<JsObject> {
    let stream = (**cx.argument::<JsBox<Stream>>(0)?).clone();

    let result = cx.empty_object();
    let id = JsBigInt::from_u64(&mut cx, stream.details.id.index());
    let is_0rtt = cx.boolean(stream.details.is_0rtt);

    result.set(&mut cx, "id", id)?;
    result.set(&mut cx, "is0rtt", is_0rtt)?;

    Ok(result)
}

fn get_remote(mut cx: FunctionContext) -> JsResult<JsString> {
    let connection = (**cx.argument::<JsBox<Connection>>(0)?).clone();

    Ok(cx.string(connection.connection.remote_address().to_string()))
}

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    cx.export_function("connect", connect)?;
    cx.export_function("create_stream", create_stream)?;
    cx.export_function("initialize_stream", initialize_stream)?;
    cx.export_function("write_stream", write_stream)?;
    cx.export_function("close_write", close_write)?;
    cx.export_function("close_stream", close_stream)?;
    cx.export_function("stream_details", stream_details)?;
    cx.export_function("get_remote", get_remote)?;
    cx.export_function("close_connection", close_connection)?;

    Ok(())
}
