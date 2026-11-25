//! Small state guards shared by the Win32 window and WebView2 callback wiring.

use std::cell::Cell;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitMessage {
    Drain,
    Restart,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Deferred {
    pub drain: bool,
    pub restart: bool,
}

/// Holds window messages that arrive while WebView2 views are being created.
#[derive(Debug, Default)]
pub(crate) struct InitGate {
    initializing: Cell<bool>,
    drain: Cell<bool>,
    restart: Cell<bool>,
}

impl InitGate {
    pub(crate) fn begin(&self) {
        self.initializing.set(true);
    }

    /// Returns true when the caller must defer the message instead of handling it.
    pub(crate) fn defer(&self, message: InitMessage) -> bool {
        if !self.initializing.get() {
            return false;
        }
        match message {
            InitMessage::Drain => self.drain.set(true),
            InitMessage::Restart => self.restart.set(true),
        }
        true
    }

    pub(crate) fn end(&self) -> Deferred {
        self.initializing.set(false);
        Deferred {
            drain: self.drain.take(),
            restart: self.restart.take(),
        }
    }
}

/// COM event registrations, removed before their controller is closed.
#[derive(Default)]
pub(crate) struct EventSubscriptions {
    removers: Vec<Box<dyn FnOnce()>>,
}

impl EventSubscriptions {
    pub(crate) fn add(&mut self, remover: impl FnOnce() + 'static) {
        self.removers.push(Box::new(remover));
    }

    pub(crate) fn clear(&mut self) {
        for remover in self.removers.drain(..) {
            remover();
        }
    }
}

impl Drop for EventSubscriptions {
    fn drop(&mut self) {
        self.clear();
    }
}

/// One owner subclass for any number of live embedded views.
#[derive(Debug)]
pub(crate) struct OwnerRefs {
    counts: HashMap<isize, usize>,
}

impl OwnerRefs {
    pub(crate) fn new() -> Self {
        Self {
            counts: HashMap::new(),
        }
    }

    /// Returns true when this is the first view of the owner.
    pub(crate) fn enter(&mut self, owner: isize) -> bool {
        let count = self.counts.entry(owner).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Returns true when the last view of the owner is gone.
    pub(crate) fn leave(&mut self, owner: isize) -> bool {
        let Some(count) = self.counts.get_mut(&owner) else {
            return false;
        };
        *count -= 1;
        if *count == 0 {
            self.counts.remove(&owner);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn messages_during_initialization_are_deferred_once() {
        let gate = InitGate::default();
        gate.begin();
        assert!(gate.defer(InitMessage::Drain));
        assert!(gate.defer(InitMessage::Drain));
        assert!(gate.defer(InitMessage::Restart));
        assert_eq!(
            gate.end(),
            Deferred {
                drain: true,
                restart: true
            }
        );
        assert!(!gate.defer(InitMessage::Drain));
        assert_eq!(gate.end(), Deferred::default());
    }

    #[test]
    fn messages_after_initialization_are_not_deferred() {
        let gate = InitGate::default();
        assert!(!gate.defer(InitMessage::Restart));
        gate.begin();
        gate.end();
        assert!(!gate.defer(InitMessage::Restart));
        assert_eq!(gate.end(), Deferred::default());
    }

    #[test]
    fn subscriptions_are_removed_once_and_on_drop() {
        let calls = Rc::new(Cell::new(0));
        let mut subscriptions = EventSubscriptions::default();
        for _ in 0..2 {
            let calls = calls.clone();
            subscriptions.add(move || calls.set(calls.get() + 1));
        }
        subscriptions.clear();
        assert_eq!(calls.get(), 2);
        subscriptions.clear();
        assert_eq!(calls.get(), 2);
        let calls_on_drop = Rc::new(Cell::new(0));
        {
            let mut subscriptions = EventSubscriptions::default();
            let calls = calls_on_drop.clone();
            subscriptions.add(move || calls.set(calls.get() + 1));
        }
        assert_eq!(calls_on_drop.get(), 1);
    }

    #[test]
    fn the_owner_subclass_follows_the_first_and_last_view() {
        let mut owners = OwnerRefs::new();
        assert!(owners.enter(7));
        assert!(!owners.enter(7));
        assert!(owners.enter(9));
        assert!(!owners.leave(7));
        assert!(owners.leave(7));
        assert!(!owners.leave(7));
        assert!(owners.leave(9));
    }
}
