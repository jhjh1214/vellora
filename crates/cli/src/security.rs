//! `vellora security`: a short answer to "is this file risky to open?", with exit codes a script
//! can act on. It runs in-process on the report of `vellora-inspect`; nothing is opened, followed or
//! run.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Value, json};
use vellora_inspect::{Features, Options, StringList, Summary, inspect};

/// Exit code when risky features were found.
pub(crate) const EXIT_RISKY: u8 = 3;

/// A feature that makes opening the file riskier, with what it means in plain words.
struct Risk {
    id: &'static str,
    title: &'static str,
    explanation: &'static str,
    present: fn(&Features) -> bool,
}

const RISKS: [Risk; 9] = [
    Risk {
        id: "open_action_javascript",
        title: "Runs a script when the file is opened",
        explanation: "JavaScript that other PDF viewers may run as soon as the document opens. Vellora never runs PDF JavaScript.",
        present: |f| f.open_action_javascript,
    },
    Risk {
        id: "javascript_actions",
        title: "Contains JavaScript actions",
        explanation: "Scripts attached to links, buttons, form fields or bookmarks.",
        present: |f| f.javascript_actions,
    },
    Risk {
        id: "names_javascript",
        title: "Has document-level scripts",
        explanation: "Scripts stored with the document itself, which some viewers run when it opens.",
        present: |f| f.names_javascript,
    },
    Risk {
        id: "additional_actions",
        title: "Reacts to events (open, close, print, focus)",
        explanation: "Actions that fire when the document is opened, closed or printed, or when a form field is entered.",
        present: |f| f.additional_actions,
    },
    Risk {
        id: "launch_actions",
        title: "Can start a program or open a file",
        explanation: "A Launch action asks a viewer to start another program or open a file on your computer.",
        present: |f| f.launch_actions,
    },
    Risk {
        id: "submit_form_actions",
        title: "Sends form data to an address",
        explanation: "A form that can send what you typed to a server.",
        present: |f| f.submit_form_actions,
    },
    Risk {
        id: "goto_remote_actions",
        title: "Opens other PDF files",
        explanation: "A link that opens another PDF document.",
        present: |f| f.goto_remote_actions,
    },
    Risk {
        id: "embedded_files",
        title: "Has embedded files",
        explanation: "Files attached inside the PDF; opening one can run whatever program handles its type.",
        present: |f| f.embedded_files,
    },
    Risk {
        id: "xfa",
        title: "Has an XFA form",
        explanation: "A form in an older XML-based format that can carry scripts.",
        present: |f| f.xfa,
    },
];

/// A feature that is worth knowing about but does not make a file risky by itself.
struct Note {
    id: &'static str,
    title: &'static str,
    present: fn(&Features) -> bool,
}

const NOTES: [Note; 4] = [
    Note {
        id: "uri_actions",
        title: "Links to addresses",
        present: |f| f.uri_actions,
    },
    Note {
        id: "acroform",
        title: "Has an interactive form",
        present: |f| f.acroform,
    },
    Note {
        id: "optional_content",
        title: "Has layers",
        present: |f| f.optional_content,
    },
    Note {
        id: "signature_fields",
        title: "Has signature fields (not verified)",
        present: |f| f.signature_fields,
    },
];

/// What the command found, ready to print.
pub(crate) struct Verdict {
    pub(crate) risky: bool,
    /// The scan could not read everything, so a clean result cannot be promised.
    pub(crate) incomplete: bool,
    pub(crate) text: String,
    pub(crate) json: Value,
}

/// Assesses `file`. The error is the message to show after `vellora: `.
pub(crate) fn assess(file: &Path, password: Option<String>) -> Result<Verdict, String> {
    let data = std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut options = Options::default();
    options.password = password;
    let summary =
        inspect(&data, &options).map_err(|error| format!("{}: {error}", file.display()))?;
    let Some(features) = summary.features else {
        return Err(format!(
            "{}: the document is encrypted; pass --password so that it can be assessed",
            file.display()
        ));
    };
    let found: Vec<&Risk> = RISKS.iter().filter(|r| (r.present)(&features)).collect();
    let notes: Vec<&Note> = NOTES.iter().filter(|n| (n.present)(&features)).collect();
    let risky = !found.is_empty();

    let json = json!({
        "risky": risky,
        "complete": features.complete,
        "findings": found.iter().map(|r| json!({
            "id": r.id, "title": r.title, "explanation": r.explanation,
        })).collect::<Vec<_>>(),
        "notes": notes.iter().map(|n| json!({"id": n.id, "title": n.title})).collect::<Vec<_>>(),
        "uris": summary.uris,
        "embedded_files": summary.embedded_files,
        "encrypted": summary.encryption.is_some(),
        "repaired": summary.repaired,
        "problems": summary.problems,
    });
    Ok(Verdict {
        risky,
        incomplete: !features.complete,
        text: render(file, &summary, &found, &notes, features.complete),
        json,
    })
}

fn render(
    file: &Path,
    summary: &Summary,
    found: &[&Risk],
    notes: &[&Note],
    complete: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "File     {}", file.display());
    if found.is_empty() {
        out.push_str("Verdict  no risky features found\n");
    } else {
        let _ = writeln!(
            out,
            "Verdict  RISKY: {} risky feature{} found",
            found.len(),
            if found.len() == 1 { "" } else { "s" }
        );
        out.push_str("\nFindings\n");
        for risk in found {
            let _ = writeln!(out, "  {}\n      {}", risk.title, risk.explanation);
        }
    }
    if !notes.is_empty() {
        out.push_str("\nAlso present\n");
        for note in notes {
            let _ = writeln!(out, "  {}", note.title);
        }
    }
    list(&mut out, "Addresses in links", summary.uris.as_ref());
    list(&mut out, "Embedded files", summary.embedded_files.as_ref());
    if summary.repaired {
        out.push_str(
            "\nThe file needed repair to be read, so the scan may have missed something.\n",
        );
    }
    if !complete {
        out.push_str(
            "\nThe scan was incomplete: a feature that is not listed may have been missed, so \
             \"no risky features\" cannot be promised.\n",
        );
    }
    out
}

/// A heading and the entries of `list`, each on one line (control characters and invisible
/// formatting from the file are shown as spaces: the file does not get to move the cursor).
fn list(out: &mut String, heading: &str, list: Option<&StringList>) {
    let Some(list) = list.filter(|list| list.total > 0) else {
        return;
    };
    let _ = writeln!(out, "\n{heading} ({})", list.total);
    for item in &list.items {
        let _ = writeln!(out, "  {}", crate::inspect::one_line(item));
    }
    if list.truncated {
        let _ = writeln!(
            out,
            "  ... and {} more",
            list.total - list.items.len() as u64
        );
    }
}
