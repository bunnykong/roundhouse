//! Answers about one `App` that several passes ask for, computed once.
//!
//! The Ruby emit's model, controller, view and library passes each ask
//! the same whole-App questions (the association class-method survey, the
//! route helpers). [`share`] holds the answers to those questions for one
//! `&App` while its guard lives: the guard borrows the App, so it cannot
//! change under an answer, and an answer is keyed by the App's address and
//! the question. Outside a guard (or for another App) [`of`] computes
//! afresh. Thread-local, like the other emit-time indexes.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::app::App;

type Answers = Vec<(&'static str, Rc<dyn Any>)>;

thread_local! {
    static SHARED: RefCell<Option<(usize, Answers)>> = const { RefCell::new(None) };
}

/// Restores the answers that were shared before it.
pub(crate) struct Guard<'a>(Option<(usize, Answers)>, std::marker::PhantomData<&'a App>);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let previous = self.0.take();
        SHARED.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Keep the answers [`of`] computes for `app` while the guard lives.
pub(crate) fn share(app: &App) -> Guard<'_> {
    let key = std::ptr::from_ref(app) as usize;
    let previous = SHARED.with(|slot| slot.borrow_mut().replace((key, Vec::new())));
    Guard(previous, std::marker::PhantomData)
}

/// The answer to `question` about `app`: `compute`d once per guard for
/// the shared App. `question` names the answer and `T` its type; the
/// computation must read nothing but `app`.
pub(crate) fn of<T: 'static>(app: &App, question: &'static str, compute: impl FnOnce() -> T) -> Rc<T> {
    let key = std::ptr::from_ref(app) as usize;
    let held = SHARED.with(|slot| match &*slot.borrow() {
        Some((k, answers)) if *k == key => answers.iter().find(|(q, _)| *q == question).map(|(_, a)| a.clone()),
        _ => None,
    });
    if let Some(answer) = held {
        return answer.downcast::<T>().unwrap_or_else(|_| panic!("app answer `{question}` has another type"));
    }
    let answer = Rc::new(compute());
    SHARED.with(|slot| {
        if let Some((k, answers)) = &mut *slot.borrow_mut()
            && *k == key
        {
            answers.push((question, answer.clone() as Rc<dyn Any>));
        }
    });
    answer
}
