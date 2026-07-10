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
mod tests;
