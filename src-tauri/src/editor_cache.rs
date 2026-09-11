//! Single-flight preparation. Slow work never holds the cache lock.
use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, Ordering}};

struct Job<K, T> {
    key: K,
    cancelled: AtomicBool,
    result: Mutex<Option<Result<Arc<T>, String>>>,
    done: Condvar,
}
pub struct LatestCache<K, T>(Mutex<Option<Arc<Job<K, T>>>>);
impl<K, T> Default for LatestCache<K, T> {
    fn default() -> Self { Self(Mutex::new(None)) }
}
impl<K: Eq, T> LatestCache<K, T> {
    pub fn prepare(&self, key: K, build: impl FnOnce(&AtomicBool) -> Result<T, String>) -> Result<Arc<T>, String> {
        let (job, owner) = {
            let mut slot = self.0.lock().map_err(|_| "编辑缓存不可用")?;
            if let Some(job) = slot.as_ref().filter(|j| j.key == key) {
                (job.clone(), false)
            } else {
                if let Some(old) = slot.as_ref() { old.cancelled.store(true, Ordering::Release); }
                let job = Arc::new(Job { key, cancelled: AtomicBool::new(false), result: Mutex::new(None), done: Condvar::new() });
                *slot = Some(job.clone());
                (job, true)
            }
        };
        if owner {
            let mut result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&job.cancelled)))
                .unwrap_or_else(|_| Err("音频准备失败，请重试".into())).map(Arc::new);
            if job.cancelled.load(Ordering::Acquire) { result = Err("编辑预载已过期".into()); }
            *job.result.lock().map_err(|_| "编辑缓存不可用")? = Some(result.clone());
            job.done.notify_all();
            if result.is_err() {
                let mut slot = self.0.lock().map_err(|_| "编辑缓存不可用")?;
                if slot.as_ref().is_some_and(|j| Arc::ptr_eq(j, &job)) { *slot = None; }
            }
            result
        } else {
            let mut result = job.result.lock().map_err(|_| "编辑缓存不可用")?;
            while result.is_none() { result = job.done.wait(result).map_err(|_| "编辑缓存不可用")?; }
            result.as_ref().unwrap().clone()
        }
    }
    pub fn ready(&self) -> Result<Option<Arc<T>>, String> {
        let job = self.0.lock().map_err(|_| "编辑缓存不可用")?.clone();
        match job {
            Some(job) => Ok(job.result.lock().map_err(|_| "编辑缓存不可用")?.as_ref().and_then(|r| r.as_ref().ok()).cloned()),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};
    #[test]
    fn current_file_does_not_wait_for_obsolete_preload() {
        let cache = Arc::new(LatestCache::default());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let old_cache = cache.clone();
        let old = thread::spawn(move || old_cache.prepare("old", |_| {
            started_tx.send(()).unwrap(); release_rx.recv().unwrap(); Ok(1)
        }));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let new_cache = cache.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let new = thread::spawn(move || { done_tx.send(new_cache.prepare("current", |_| Ok(2))).unwrap(); });
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        assert_eq!(*result.unwrap().unwrap(), 2);
        assert!(old.join().unwrap().is_err()); new.join().unwrap();
        assert_eq!(*cache.ready().unwrap().unwrap(), 2);
    }
    #[test]
    fn same_content_reuses_and_changed_content_invalidates() {
        let cache = LatestCache::default();
        let a = cache.prepare(vec![1,2], |_| Ok(7)).unwrap();
        let hit = cache.prepare(vec![1,2], |_| panic!("decoded twice")).unwrap();
        assert!(Arc::ptr_eq(&a, &hit));
        assert_eq!(*cache.prepare(vec![1,3], |_| Ok(8)).unwrap(), 8);
        assert!(cache.prepare(vec![4], |_| Err("failed".into())).is_err());
        assert_eq!(*cache.prepare(vec![4], |_| Ok(9)).unwrap(), 9);
    }
}
