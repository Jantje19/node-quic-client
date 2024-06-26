use std::{net::SocketAddr, sync::Arc};

use neon::prelude::*;
use once_cell::sync::OnceCell;
use quinn::SendStream;
use tokio::{runtime::Runtime, sync::Mutex, task::JoinHandle};

mod quic;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();

// Return a global tokio runtime or create one if it doesn't exist.
// Throws a JavaScript exception if the `Runtime` fails to create.
fn runtime<'a, C: Context<'a>>(cx: &mut C) -> NeonResult<&'static Runtime> {
    RUNTIME.get_or_try_init(|| Runtime::new().or_else(|err| cx.throw_error(err.to_string())))
}

#[derive(Clone)]
struct Connection {
    connection: Arc<quinn::Connection>,
    join_handle: Arc<JoinHandle<()>>,
}

impl Finalize for Connection {
    fn finalize<'a, C: Context<'a>>(self, _: &mut C) {
        self.join_handle.abort();
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
    let on_close = cx.argument::<JsFunction>(3)?.root(&mut cx);
    let alpn_protocols: Option<Handle<JsArray>> = cx.argument::<JsValue>(4)?.downcast(&mut cx).ok();
    let certificate_authorities: Option<Handle<JsArray>> =
        cx.argument::<JsValue>(5)?.downcast(&mut cx).ok();

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

    let on_close_channel = cx.channel();

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

            let handle = {
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

            Ok(cx.boxed(Connection {
                connection,
                join_handle: Arc::new(handle),
            }))
        });
    });

    Ok(promise)
}

#[derive(Clone)]
struct Stream {
    send: Arc<Mutex<SendStream>>,
    handle: Arc<JoinHandle<()>>,
}

impl Finalize for Stream {
    fn finalize<'a, C: Context<'a>>(self, _: &mut C) {
        let rt = RUNTIME.get().unwrap();

        self.handle.abort();

        rt.spawn(async move {
            let _ = self.send.lock().await.finish().await;
        });
    }
}

async fn handle_read(
    mut recv: quinn::RecvStream,
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
        match recv.read(&mut buf).await {
            Err(e) => match e {
                quinn::ReadError::ConnectionLost(e) => {
                    handle_close(e.to_string());
                    return;
                }
                quinn::ReadError::UnknownStream | quinn::ReadError::Reset(_) => {
                    handle_close(e.to_string());
                    return;
                }
                quinn::ReadError::IllegalOrderedRead | quinn::ReadError::ZeroRttRejected => {
                    let callback = error_callback.clone();
                    error.1.send(move |mut cx| {
                        let callback = callback.as_ref().clone(&mut cx).into_inner(&mut cx);
                        let this = cx.undefined();

                        let args = vec![cx.error(e.to_string()).unwrap().upcast()];

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
                            let a = JsInt8Array::new(&mut cx, packet.len())?;
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
    let on_data = cx.argument::<JsFunction>(1)?.root(&mut cx);
    let on_close = cx.argument::<JsFunction>(2)?.root(&mut cx);
    let on_error = cx.argument::<JsFunction>(3)?.root(&mut cx);

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    let data_channel = cx.channel();
    let close_channel = cx.channel();
    let error_channel = cx.channel();

    rt.spawn(async move {
        let result = connection.connection.open_bi().await;

        deferred.settle_with(&channel, move |mut cx| {
            let (send, recv) = result.or_else(|err| cx.throw_error(err.to_string()))?;

            let handle = rt.spawn(async move {
                handle_read(
                    recv,
                    (on_data, data_channel),
                    (on_close, close_channel),
                    (on_error, error_channel),
                )
                .await
            });

            let stream = Stream {
                send: Arc::new(Mutex::new(send)),
                handle: Arc::new(handle),
            };

            Ok(cx.boxed(stream))
        });
    });

    Ok(promise)
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
            let mut send = stream.send.lock().await;

            send.write_all(&packet).await
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

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        let result = stream.send.lock().await.finish().await;

        deferred.settle_with(&channel, move |mut cx| {
            result.or_else(|err| cx.throw_error(err.to_string()))?;

            Ok(cx.undefined())
        });
    });

    Ok(promise)
}

fn close_write(mut cx: FunctionContext) -> JsResult<JsPromise> {
    let stream = (**cx.argument::<JsBox<Stream>>(0)?).clone();

    let rt = runtime(&mut cx)?;

    let channel = cx.channel();
    let (deferred, promise) = cx.promise();

    rt.spawn(async move {
        let result = stream.send.lock().await.finish().await;

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

fn get_remote(mut cx: FunctionContext) -> JsResult<JsString> {
    let connection = (**cx.argument::<JsBox<Connection>>(0)?).clone();

    Ok(cx.string(connection.connection.remote_address().to_string()))
}

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    cx.export_function("connect", connect)?;
    cx.export_function("create_stream", create_stream)?;
    cx.export_function("write_stream", write_stream)?;
    cx.export_function("close_write", close_write)?;
    cx.export_function("close_stream", close_stream)?;
    cx.export_function("get_remote", get_remote)?;
    cx.export_function("close_connection", close_connection)?;

    Ok(())
}
