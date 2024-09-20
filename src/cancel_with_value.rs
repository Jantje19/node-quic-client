use std::sync::{Arc, RwLock};

use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct CancelWithValue<T: Clone> {
    token: CancellationToken,
    value: Arc<RwLock<T>>,
}

impl<T: Clone + Default> CancelWithValue<T> {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            value: Arc::new(RwLock::new(T::default())),
        }
    }
}

impl<T: Clone> CancelWithValue<T> {
    pub fn cancel(&self, value: T) {
        *self.value.write().unwrap() = value;
        self.token.cancel();
    }

    pub async fn cancelled(&self) -> T {
        self.token.cancelled().await;
        self.value.read().unwrap().clone()
    }
}
