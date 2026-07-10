use super::*;

#[test]
fn domain_event_sender_sends_events() {
    let (sender, mut rx) = DomainEventSender::new();
    sender.send(DomainEvent::LiveCountDirty);
    assert!(matches!(rx.try_recv(), Ok(DomainEvent::LiveCountDirty)));
}
