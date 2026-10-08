//! `vellora inspect`: runs the analysis of `vellora-inspect` and prints it.

use std::fmt::Write as _;
use std::path::Path;

use vellora_inspect::{Features, Options, Permissions, Summary, inspect};

/// Reads `file`, inspects it and prints the report to stdout (a note about a locked document goes
/// to stderr). The error is the message to show after `vellora: `.
pub(crate) fn run(file: &Path, json: bool, password: Option<String>) -> Result<(), String> {
    // The whole file is read: `cos` works on a byte slice, and inspecting needs random access.
    let data = std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut options = Options::default();
    options.password = password;
    let summary =
        inspect(&data, &options).map_err(|error| format!("{}: {error}", file.display()))?;

    if json {
        let text = serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?;
        println!("{text}");
    } else {
        print!("{}", render(file, &summary));
    }
    if summary.locked {
        eprintln!("vellora: the file is encrypted; pass --password to read its pages and features");
    }
    Ok(())
}

/// The table printed without `--json`.
pub(crate) fn render(file: &Path, summary: &Summary) -> String {
    let mut out = String::new();
    let mut row = |label: &str, value: &str| {
        // Writing to a String cannot fail.
        let _ = writeln!(out, "{label:<14}{value}");
    };
    row("File", &file.display().to_string());
    row("Size", &format!("{} bytes", summary.file_size));
    row(
        "PDF version",
        summary.version.as_deref().unwrap_or("unknown"),
    );
    row(
        "Pages",
        &summary
            .pages
            .map_or("unknown (locked)".to_owned(), |n| n.to_string()),
    );
    row("Objects", &summary.objects.to_string());
    row("Revisions", &summary.revisions.to_string());
    row(
        "Repaired",
        &if summary.repaired {
            format!("yes ({})", summary.repair_reasons.join("; "))
        } else {
            "no".to_owned()
        },
    );
    match &summary.encryption {
        None => row("Encryption", "none"),
        Some(e) => {
            row(
                "Encryption",
                &format!(
                    "{} handler, V{} R{}, {}-bit key, streams {}, strings {}{}",
                    e.handler,
                    e.version,
                    e.revision,
                    e.key_bits,
                    e.stream_method,
                    e.string_method,
                    if e.encrypt_metadata {
                        ""
                    } else {
                        ", metadata in the clear"
                    },
                ),
            );
            row(
                "Unlocked with",
                e.unlocked_with
                    .map_or("nothing (password required)", |role| role),
            );
            row("Permissions", &permissions(e.permissions, e.allowed));
        }
    }
    if let Some(features) = &summary.features {
        out.push_str("\nFeatures\n");
        for (label, present) in feature_rows(features) {
            let _ = writeln!(out, "  {label:<32}{}", if present { "yes" } else { "no" });
        }
        if !features.complete {
            out.push_str("  (the scan was incomplete: a \"no\" may be a miss)\n");
        }
    }
    if !summary.problems.is_empty() {
        out.push_str("\nProblems\n");
        for problem in &summary.problems {
            let _ = writeln!(out, "  {problem}");
        }
    }
    out
}

fn permissions(mask: i32, allowed: Permissions) -> String {
    let names = [
        (allowed.print, "print"),
        (allowed.modify, "modify"),
        (allowed.copy, "copy"),
        (allowed.annotate, "annotate"),
        (allowed.fill_forms, "fill forms"),
        (allowed.accessibility, "accessibility"),
        (allowed.assemble, "assemble"),
        (allowed.print_high_quality, "print high quality"),
    ];
    let granted: Vec<&str> = names
        .iter()
        .filter(|(on, _)| *on)
        .map(|&(_, name)| name)
        .collect();
    let granted = if granted.is_empty() {
        "none".to_owned()
    } else {
        granted.join(", ")
    };
    format!("{granted} (P = {mask})")
}

fn feature_rows(f: &Features) -> [(&'static str, bool); 13] {
    [
        ("JavaScript on open", f.open_action_javascript),
        ("JavaScript actions", f.javascript_actions),
        ("Document-level JavaScript", f.names_javascript),
        ("Additional actions (/AA)", f.additional_actions),
        ("Launch actions", f.launch_actions),
        ("URI actions", f.uri_actions),
        ("SubmitForm actions", f.submit_form_actions),
        ("GoToR actions", f.goto_remote_actions),
        ("Embedded files", f.embedded_files),
        ("AcroForm", f.acroform),
        ("XFA", f.xfa),
        ("Optional content (layers)", f.optional_content),
        ("Signature fields", f.signature_fields),
    ]
}
