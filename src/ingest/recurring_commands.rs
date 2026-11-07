//! Solid Queue's recurring `command:` tasks, compiled from
//! `config/recurring.yml` instead of evaluated.
//!
//! A task written as `command: ReadRibbon.expire_old_ribbons!` is enqueued
//! as `SolidQueue::RecurringJob`, whose `perform(command)` is `eval(command)`
//! (solid_queue 1.x). An ahead-of-time target has no `eval`, so when the app
//! carries solid_queue's source the method becomes a `case` over every
//! command the file names, in any environment, each arm the command's own
//! code. A command that is not in the file when the app is built (one a
//! program creates with `SolidQueue::RecurringTask.create(command: ...)`)
//! raises instead of running.
//!
//! A file with ERB is left alone, with `eval` and its refusal intact: its
//! commands are not known until Rails renders it.
//!
//! Only the default path is read. A schedule chosen when the app starts
//! (`SOLID_QUEUE_RECURRING_SCHEDULE`, `--recurring_schedule_file`) is not
//! known at build time, and its commands raise unless this file names them
//! too: a known gap.

use std::path::Path;

use super::{IngestError, IngestResult};
use crate::vfs::Vfs;

const JOB: &str = "SolidQueue::RecurringJob";

/// The command strings of `config/recurring.yml`, in file order, once each.
fn commands<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Option<Vec<String>>> {
    let path = dir.join("config").join("recurring.yml");
    let Ok(source) = vfs.read(&path) else { return Ok(None) };
    let text = String::from_utf8_lossy(&source);
    if text.contains("<%") {
        return Ok(None);
    }
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).map_err(|e| IngestError::Unsupported {
        file: "config/recurring.yml".to_string(),
        message: format!("does not parse as YAML: {e}"),
    })?;
    let mut out: Vec<String> = Vec::new();
    let Some(envs) = doc.as_mapping() else { return Ok(Some(out)) };
    for tasks in envs.values() {
        let Some(tasks) = tasks.as_mapping() else { continue };
        for task in tasks.values() {
            let Some(command) = task.get("command").and_then(|c| c.as_str()) else { continue };
            if !out.iter().any(|c| c == command) {
                out.push(command.to_string());
            }
        }
    }
    Ok(Some(out))
}

/// The `perform` that runs each known command as code.
fn perform_source(commands: &[String]) -> String {
    if commands.is_empty() {
        return String::from(
            "def perform(command)\n  raise ArgumentError, \"recurring command \" + command.inspect + \" is not in config/recurring.yml as built\"\nend\n",
        );
    }
    let mut src = String::from("def perform(command)\n  case command\n");
    for command in commands {
        src.push_str(&format!("  when {:?}\n    {}\n", command, command));
    }
    src.push_str(
        "  else\n    raise ArgumentError, \"recurring command \" + command.inspect + \" is not in config/recurring.yml as built\"\n  end\nend\n",
    );
    src
}

pub(crate) fn apply<V: Vfs + ?Sized>(app: &mut crate::App, vfs: &V, dir: &Path) -> IngestResult<()> {
    let Some(lc) = app.library_classes.iter_mut().find(|lc| lc.name.0.as_str() == JOB) else { return Ok(()) };
    let Some(commands) = commands(vfs, dir)? else { return Ok(()) };
    // Not spliced when one fails to parse: that command is the app's, and losing it would be a task that silently never runs.
    for command in &commands {
        let parsed = super::prism::parse_silent(command.as_bytes());
        if parsed.errors().next().is_some() {
            return Err(IngestError::Unsupported {
                file: "config/recurring.yml".to_string(),
                message: format!("recurring command {command:?} does not parse as Ruby"),
            });
        }
    }
    let synth = crate::runtime_src::parse_methods(&perform_source(&commands)).map_err(|message| IngestError::Unsupported {
        file: "config/recurring.yml".to_string(),
        message: format!("the recurring commands do not parse once synthesized: {message}"),
    })?;
    let Some(perform) = synth.into_iter().next() else { return Ok(()) };
    if let Some(slot) = lc.methods.iter_mut().find(|m| m.name.as_str() == "perform" && m.receiver == crate::dialect::MethodReceiver::Instance) {
        let visibility = slot.visibility;
        *slot = perform;
        slot.visibility = visibility;
    }
    Ok(())
}
