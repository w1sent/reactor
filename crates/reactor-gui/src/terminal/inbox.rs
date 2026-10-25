//! Where the engine's thread leaves events for the panel, and how the panel is woken for them.
//!
//! A queue and a waker, no runtime: the panel awaits [`Inbox::ready`] on gpui's executor, the
//! engine's thread calls [`Inbox::push`]. Nothing polls on a timer, so a keystroke's echo is drawn
//! as soon as the engine has parsed it.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use super::types::Event;

#[derive(Default)]
struct State {
    queue: VecDeque<Event>,
    waker: Option<Waker>,
}

#[derive(Default)]
pub struct Inbox {
    state: Mutex<State>,
}

impl Inbox {
    /// Leave an event. Repeated wakeups collapse into one: a flood of output is one redraw
    /// pending, not a queue of them.
    pub fn push(&self, event: Event) {
        let waker = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(event, Event::Wakeup) && matches!(state.queue.back(), Some(Event::Wakeup)) {
                return;
            }
            state.queue.push_back(event);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Take up to `max` events, in the order they were left.
    pub fn take(&self, max: usize) -> Vec<Event> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let n = state.queue.len().min(max);
        state.queue.drain(..n).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .is_empty()
    }

    /// Completes when there is at least one event. One waiter at a time.
    pub fn ready(&self) -> Ready<'_> {
        Ready { inbox: self }
    }
}

pub struct Ready<'a> {
    inbox: &'a Inbox,
}

impl Future for Ready<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.inbox.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.queue.is_empty() {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Wake;

    struct Flag(AtomicBool);
    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn repeated_wakeups_are_one_event_and_order_is_kept() {
        let inbox = Inbox::default();
        inbox.push(Event::Wakeup);
        inbox.push(Event::Wakeup);
        inbox.push(Event::PtyWrite("a".into()));
        inbox.push(Event::PtyWrite("b".into()));
        inbox.push(Event::Wakeup);
        let got = inbox.take(10);
        assert_eq!(got.len(), 4);
        assert!(matches!(&got[1], Event::PtyWrite(s) if s == "a"));
        assert!(matches!(&got[2], Event::PtyWrite(s) if s == "b"));
        assert!(inbox.is_empty());
    }

    #[test]
    fn take_is_bounded() {
        let inbox = Inbox::default();
        for i in 0..5 {
            inbox.push(Event::PtyWrite(i.to_string()));
        }
        assert_eq!(inbox.take(2).len(), 2);
        assert_eq!(inbox.take(10).len(), 3);
    }

    #[test]
    fn the_waiter_is_woken_by_a_push() {
        let inbox = Inbox::default();
        let flag = Arc::new(Flag(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let mut cx = Context::from_waker(&waker);
        let mut ready = inbox.ready();
        assert_eq!(Pin::new(&mut ready).poll(&mut cx), Poll::Pending);
        assert!(!flag.0.load(Ordering::SeqCst));
        inbox.push(Event::Bell);
        assert!(flag.0.load(Ordering::SeqCst));
        assert_eq!(Pin::new(&mut ready).poll(&mut cx), Poll::Ready(()));
    }
}
