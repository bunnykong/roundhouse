// Not on ActiveJob::Base's own singleton: an engine's reopen (`def self.queue_adapter; current_queue_adapter || super; end`) replaces that, and its `super` reaches this module, as Rails' QueueAdapter::ClassMethods.
use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

pub(super) fn synthesize(app: &mut App) {
    let base = ClassId(Symbol::from("ActiveJob::Base"));
    if !app.library_classes.iter().any(|lc| lc.name == base) {
        return;
    }
    // Not SolidQueueAdapter without solid_queue's models: only the runtime that runs them defines it (`project::drop_solid_queue_standin`).
    let solid_queue = app.models.iter().any(|m| m.name.0.as_str() == "SolidQueue::Job");
    let (adapter, name) = match app.queue_adapter.as_deref() {
        None | Some("async") => ("AsyncAdapter", "async"),
        Some("solid_queue") if solid_queue => ("SolidQueueAdapter", "solid_queue"),
        Some(_) => return,
    };
    let source = format!(
        "module ActiveJob::QueueAdapter::ClassMethods\n  def queue_adapter\n    @queue_adapter ||= ActiveJob::QueueAdapters::{adapter}.new\n  end\n\n  def queue_adapter_name\n    case queue_adapter\n    when ActiveJob::QueueAdapters::{adapter} then \"{name}\"\n    end\n  end\nend\n\n\
         module ActiveJob::Enqueuing\n  def enqueue(options = {{}})\n    set(options)\n    @successfully_enqueued = false\n    run_callbacks(:enqueue) do\n      scheduled = @scheduled_at\n      if scheduled.nil?\n        self.class.queue_adapter.enqueue(self)\n      else\n        self.class.queue_adapter.enqueue_at(self, scheduled.to_f)\n      end\n      @successfully_enqueued = true\n    rescue ActiveJob::EnqueueError => e\n      @enqueue_error = e\n    end\n    successfully_enqueued? ? self : false\n  end\nend\n"
    );
    let Ok(classes) = super::library_class::ingest_library_classes(source.as_bytes(), "<queue_adapter>") else { return };
    app.library_classes.extend(classes);
    let module = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Const { path: ["ActiveJob", "QueueAdapter", "ClassMethods"].into_iter().map(Symbol::from).collect() },
    );
    // Rails' `enqueue` beside the adapter it hands the job to: an app without one has neither.
    let enqueuing = ClassId(Symbol::from("ActiveJob::Enqueuing"));
    if let Some(lc) = app.library_classes.iter_mut().find(|lc| lc.name == base) {
        lc.unknown_calls.push(Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Send { recv: None, method: Symbol::from("extend"), args: vec![module], block: None, parenthesized: false },
        ));
        lc.includes.push(enqueuing);
        lc.include_spans.push(crate::span::Span::synthetic());
    }
}
