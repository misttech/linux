// SPDX-License-Identifier: GPL-2.0

//! Scope-exit actions.

/// Runs an action when dropped, unless cancelled.
#[must_use = "if unused the deferred action runs immediately"]
pub struct Deferred<F: FnOnce()> {
    action: Option<F>,
}

impl<F: FnOnce()> Deferred<F> {
    /// Creates a guard that runs `action` on drop.
    #[inline]
    pub const fn new(action: F) -> Self {
        Self {
            action: Some(action),
        }
    }

    /// Cancels the action.
    #[inline]
    pub fn cancel(&mut self) {
        self.action = None;
    }

    /// Runs the action now; it will not run again on drop.
    #[inline]
    pub fn call(&mut self) {
        if let Some(action) = self.action.take() {
            action();
        }
    }
}

impl<F: FnOnce()> Drop for Deferred<F> {
    #[inline]
    fn drop(&mut self) {
        self.call();
    }
}

/// Runs `action` when the returned guard is dropped.
#[inline]
pub fn defer<F: FnOnce()>(action: F) -> Deferred<F> {
    Deferred::new(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_on_drop() {
        let mut ran = false;
        {
            let _guard = defer(|| ran = true);
        }
        assert!(ran);
    }

    #[test]
    fn cancel_prevents_run() {
        let mut ran = false;
        {
            let mut guard = defer(|| ran = true);
            guard.cancel();
        }
        assert!(!ran);
    }

    #[test]
    fn call_runs_once() {
        let mut count = 0;
        {
            let mut guard = defer(|| count += 1);
            guard.call();
        }
        assert_eq!(count, 1);
    }
}
