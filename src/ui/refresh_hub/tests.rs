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
