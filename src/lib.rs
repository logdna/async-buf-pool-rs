use async_channel::unbounded;
use async_channel::{Receiver, Sender};
use thiserror::Error;

use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

#[derive(Error, std::fmt::Debug)]
pub enum PoolError {
    #[error("Failed to attach object to pool")]
    AttachError,
    #[error("No buffers available in pool")]
    NoBuffersAvailable,
}

#[derive(Clone)]
pub struct Pool<F, T> {
    object_bucket: Receiver<T>,
    object_return: Sender<T>,
    extend_fn: F,
}

impl<F, T: std::marker::Send> Pool<Arc<F>, T>
where
    F: Fn() -> T + std::marker::Send + std::marker::Sync + 'static + ?Sized,
{
    #[inline]
    pub fn new(initial_capacity: usize, init: Arc<F>) -> Self {
        let (s, r) = unbounded();

        for _ in 0..initial_capacity {
            s.try_send(init()).expect("Pool is closed");
        }

        Pool {
            object_bucket: r,
            object_return: s,
            extend_fn: init,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.object_bucket.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.object_bucket.is_empty()
    }

    #[inline]
    pub async fn pull(&self) -> Option<Reusable<T>> {
        self.object_bucket
            .recv()
            .await
            .ok()
            .map(|data| Reusable::new(self.object_return.clone(), data))
    }

    #[inline]
    pub fn try_pull(&self) -> Result<Reusable<T>, PoolError> {
        self.object_bucket
            .try_recv()
            .map(|data| Reusable::new(self.object_return.clone(), data))
            .map_err(|_| /*TODO handle the real errors*/ PoolError::NoBuffersAvailable)
    }

    #[inline]
    pub async fn attach(&self, t: T) -> Result<(), PoolError> {
        self.object_return
            .send(t)
            .await
            .map_err(|_| PoolError::AttachError)
    }

    #[inline]
    pub fn try_attach(&self, t: T) -> Result<(), PoolError> {
        self.object_return
            .try_send(t)
            .map_err(|_| PoolError::AttachError)
    }

    #[inline]
    pub fn expand(&mut self) -> Result<(), PoolError> {
        self.try_attach((self.extend_fn)())
    }
}

pub struct Reusable<T> {
    pool: Sender<T>,
    data: ManuallyDrop<T>,
}

impl<'a, T> Reusable<T> {
    #[inline]
    pub fn new(pool: Sender<T>, t: T) -> Self {
        Self {
            pool,
            data: ManuallyDrop::new(t),
        }
    }

    #[inline]
    pub fn detach(mut self) -> (Sender<T>, T) {
        (self.pool.clone(), unsafe { self.take() })
    }

    unsafe fn take(&mut self) -> T {
        ManuallyDrop::take(&mut self.data)
    }
}

impl<'a, T> Deref for Reusable<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<'a, T> DerefMut for Reusable<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<'a, T> Drop for Reusable<T> {
    #[inline]
    fn drop(&mut self) {
        let obj = unsafe { self.take() };
        // If we can't put it back on the pool drop it
        match self.pool.try_send(obj) {
            Ok(_) => {}
            Err(e) => drop(e),
        };
    }
}
