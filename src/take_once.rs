use std::sync::Mutex;

pub struct TakeOnce<T: Send + Sync> {
    value: Mutex<Option<T>>,
}

impl<T> TakeOnce<T>
where
    T: Send + Sync,
{
    pub fn new(value: T) -> Self {
        Self {
            value: Mutex::new(Some(value)),
        }
    }

    pub fn peek<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.value.lock().unwrap();

        guard.as_ref().map(f).unwrap()
    }

    pub fn take(&self) -> T {
        self.value.lock().unwrap().take().unwrap()
    }
}
