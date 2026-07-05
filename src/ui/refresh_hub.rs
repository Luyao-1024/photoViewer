use crate::core::events::DomainEvent;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiRefreshScope {
    Photos,
    Sidebar,
    TrashPage(u64),
    AlbumDetail(String),
    Viewer(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionToken(u64);

type RefreshCallback = Rc<dyn Fn(&DomainEvent)>;

#[derive(Clone)]
pub struct UiRefreshHub {
    next_token: Rc<Cell<u64>>,
    subscribers: Rc<RefCell<BTreeMap<SubscriptionToken, (UiRefreshScope, RefreshCallback)>>>,
}

impl UiRefreshHub {
    pub fn new() -> Self {
        Self {
            next_token: Rc::new(Cell::new(1)),
            subscribers: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    pub fn subscribe(&self, scope: UiRefreshScope, callback: RefreshCallback) -> SubscriptionToken {
        let token = SubscriptionToken(self.next_token.get());
        self.next_token.set(token.0.saturating_add(1));
        self.subscribers
            .borrow_mut()
            .insert(token, (scope, callback));
        token
    }

    pub fn unsubscribe(&self, token: SubscriptionToken) {
        self.subscribers.borrow_mut().remove(&token);
    }

    pub fn dispatch(&self, event: &DomainEvent) {
        let callbacks = self
            .subscribers
            .borrow()
            .values()
            .map(|(_, callback)| callback.clone())
            .collect::<Vec<_>>();
        for callback in callbacks {
            callback(event);
        }
    }

    #[cfg(test)]
    fn subscriber_count(&self) -> usize {
        self.subscribers.borrow().len()
    }
}

impl Default for UiRefreshHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{ChangeSource, DomainEvent};

    #[test]
    fn dispatch_invokes_subscribers_until_unsubscribed() {
        let hub = UiRefreshHub::new();
        let calls = Rc::new(Cell::new(0));
        let calls_for_sub = calls.clone();
        let token = hub.subscribe(
            UiRefreshScope::Photos,
            Rc::new(move |_| calls_for_sub.set(calls_for_sub.get() + 1)),
        );

        hub.dispatch(&DomainEvent::LiveCountDirty);
        assert_eq!(calls.get(), 1);
        assert_eq!(hub.subscriber_count(), 1);

        hub.unsubscribe(token);
        hub.dispatch(&DomainEvent::TrashChanged {
            source: ChangeSource::TrashReconcile,
        });
        assert_eq!(calls.get(), 1);
        assert_eq!(hub.subscriber_count(), 0);
    }
}
