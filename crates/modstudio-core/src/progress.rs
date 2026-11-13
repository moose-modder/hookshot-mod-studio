//! What a long task (a new project, a build) tells the user as it goes: steps, lines and warnings, for the
//! CLI's output and the app's log view. A task can be cancelled; it stops at the next step or while a tool runs.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::{Error, Result};

/// The error message of a cancelled task.
pub const CANCELLED: &str = "Cancelled";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "lowercase")]
pub enum Event {
    /// A new step ("Reading the game's model").
    Step(String),
    /// A line of the step's output.
    Line(String),
    /// Something the user should look at; the task goes on.
    Warning(String),
}

pub struct Progress<'a> {
    sink: Box<dyn FnMut(Event) + 'a>,
    cancel: Option<&'a AtomicBool>,
}

impl<'a> Progress<'a> {
    pub fn new(sink: impl FnMut(Event) + 'a) -> Self {
        Progress {
            sink: Box::new(sink),
            cancel: None,
        }
    }

    /// Printed as it comes (the CLI).
    pub fn stdout() -> Progress<'static> {
        Progress::new(|e| match e {
            Event::Step(s) => println!("\n== {s}"),
            Event::Line(s) => println!("   {s}"),
            Event::Warning(s) => println!("   Warning: {s}"),
        })
    }

    /// The task stops when `flag` is set.
    pub fn with_cancel(mut self, flag: &'a AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }

    pub fn step(&mut self, text: impl Into<String>) {
        (self.sink)(Event::Step(text.into()));
    }

    pub fn line(&mut self, text: impl Into<String>) {
        (self.sink)(Event::Line(text.into()));
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        (self.sink)(Event::Warning(text.into()));
    }

    /// Passes everything on with each line changed by `map` (and the same cancel flag).
    pub fn map_lines<'b>(&'b mut self, map: impl Fn(String) -> String + 'b) -> Progress<'b>
    where
        'a: 'b,
    {
        let cancel = self.cancel;
        let sink = &mut self.sink;
        Progress {
            sink: Box::new(move |e| match e {
                Event::Line(l) => sink(Event::Line(map(l))),
                other => sink(other),
            }),
            cancel,
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// An error if the task was cancelled.
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(Error(CANCELLED.into()))
        } else {
            Ok(())
        }
    }
}
