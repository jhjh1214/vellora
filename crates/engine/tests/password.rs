//! M1 task 7: encrypted documents through the real engine executable. The password travels in
//! `Open`; a document that needs one is refused with a typed answer, a wrong password is refused
//! with another, and neither shows content or ends the session.
//!
//! The fixtures are the qpdf-made files of `crates/cos/tests/fixtures/encryption/` (user password
//! `user-pw`, owner password `owner-pw`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::process::Stdio;

use support::Session;
use vellora_ipc::{ErrorKind, Priority, Request, RequestId, Response, SlotId, TileRect};
use vellora_shm::SlotGeometry;

const USER: &str = "user-pw";
const OWNER: &str = "owner-pw";

/// One revision of each cipher family: RC4 40, RC4 128, AES-128 and AES-256.
const PROTECTED: [&str; 4] = [
    "r2-rc4-40-user-password",
    "r3-rc4-128-user-password",
    "r4-aes-128-user-password",
    "r6-aes-256-user-password",
];

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cos/tests/fixtures/encryption")
        .join(format!("{name}.pdf"));
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn started(name: &str) -> Session {
    let mut engine = Session::start(&fixture(name));
    engine.handshake();
    engine
}

fn refusal(response: Response) -> (Option<RequestId>, ErrorKind, String) {
    match response {
        Response::Error {
            req_id,
            kind,
            message,
        } => (req_id, kind, message),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Asks for a tile of page 1 and says whether the engine delivered it.
fn renders_page_one(engine: &mut Session) -> bool {
    engine.send(&Request::RenderTile {
        req_id: RequestId(1),
        page: 0,
        scale: 1.0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        },
        slot: SlotId(0),
        priority: Priority::Visible,
    });
    matches!(engine.recv(), Response::TileReady { .. })
}

#[test]
fn a_document_with_a_user_password_is_refused_until_it_is_given() {
    for name in PROTECTED {
        let mut engine = started(name);
        let (request, kind, message) = refusal(engine.open());
        assert_eq!(
            (request, kind),
            (None, ErrorKind::PasswordRequired),
            "{name}: {message}"
        );
        assert!(message.contains("password required"), "{message}");

        // Nothing is open, so nothing can be rendered.
        engine.send(&Request::RenderTile {
            req_id: RequestId(1),
            page: 0,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            slot: SlotId(0),
            priority: Priority::Visible,
        });
        let (_, kind, message) = refusal(engine.recv());
        assert_eq!(kind, ErrorKind::InvalidRequest, "{name}");
        assert!(message.contains("no document"), "{message}");
        assert!(
            engine.slot(0).iter().all(|&byte| byte == 0),
            "{name}: no pixels"
        );
    }
}

#[test]
fn a_wrong_password_is_refused_again_and_again_without_ending_the_session() {
    for name in PROTECTED {
        let mut engine = started(name);
        for wrong in [
            "wrong",
            "",
            "user-p",
            "USER-PW",
            "user-pw ",
            "owner",
            "ünïcode ✓",
        ] {
            let (request, kind, message) = refusal(engine.open_with_password(Some(wrong)));
            assert_eq!(
                (request, kind),
                (None, ErrorKind::WrongPassword),
                "{name} {wrong:?}: {message}"
            );
            assert!(message.contains("incorrect password"), "{message}");
            assert!(
                !message.contains(wrong) || wrong.is_empty(),
                "the answer must not echo the password: {message}"
            );
        }
        // Still alive, still closed: the right password opens it afterwards.
        let Response::Opened { page_count, .. } = engine.open_with_password(Some(USER)) else {
            panic!("{name}: the user password must open it");
        };
        assert_eq!(page_count, 1, "{name}");
        assert!(renders_page_one(&mut engine), "{name}");
    }
}

#[test]
fn the_user_and_the_owner_password_both_open_a_protected_document() {
    for name in PROTECTED {
        for password in [USER, OWNER] {
            let mut engine = started(name);
            let Response::Opened {
                page_count,
                repairs,
                ..
            } = engine.open_with_password(Some(password))
            else {
                panic!("{name}: {password} must open it");
            };
            assert_eq!(page_count, 1, "{name} {password}");
            // `cos` unlocked it with the same password, so there is nothing to report.
            assert!(repairs.is_empty(), "{name} {password}: {repairs:?}");
            assert!(renders_page_one(&mut engine), "{name} {password}");
            // A document that is open cannot be opened again, with or without a password.
            let (_, kind, message) = refusal(engine.open_with_password(Some(password)));
            assert_eq!(kind, ErrorKind::InvalidRequest, "{name}");
            assert!(message.contains("already open"), "{message}");
        }
    }
}

#[test]
fn a_document_with_only_an_owner_password_opens_without_asking() {
    for name in [
        "r2-rc4-40",
        "r3-rc4-128",
        "r4-rc4-128",
        "r4-aes-128",
        "r4-aes-128-no-metadata",
        "r6-aes-256",
    ] {
        let mut engine = started(name);
        let Response::Opened {
            page_count,
            repairs,
            ..
        } = engine.open()
        else {
            panic!("{name}: permissions alone must not stop a viewer");
        };
        assert_eq!(page_count, 1, "{name}");
        assert!(repairs.is_empty(), "{name}: {repairs:?}");
        assert!(renders_page_one(&mut engine), "{name}");
    }
}

#[test]
fn a_password_for_a_document_that_needs_none_is_ignored() {
    let mut engine = Session::start(support::GOLDEN_PDF);
    engine.handshake();
    let Response::Opened { page_count, .. } = engine.open_with_password(Some("not needed")) else {
        panic!("an unencrypted document opens whatever is sent");
    };
    assert_eq!(page_count, 3);
}

/// Acceptance: the password appears in no log, at the most verbose level. The engine is started
/// with `VELLORA_LOG=trace`, put through every kind of answer, and its whole standard error is
/// searched for the passwords.
#[test]
fn passwords_never_appear_in_the_engine_log() {
    let mut log = tempfile::tempfile().unwrap();
    let stderr = Stdio::from(log.try_clone().unwrap());
    let mut engine = Session::start_logged(
        &fixture("r6-aes-256-user-password"),
        SlotGeometry::new(2, 1 << 20).unwrap(),
        None,
        stderr,
        Some("trace"),
    );
    engine.handshake();

    // Distinctive, so that a match cannot be an accident; one of them is not ASCII, which makes
    // the engine try two forms of it.
    let wrong = ["hunter2-WRONG-πασσ", "tr0ub4dor&3-WRONG", "wrong-ünï-😀"];
    refusal(engine.open());
    for password in wrong {
        refusal(engine.open_with_password(Some(password)));
    }
    assert!(matches!(
        engine.open_with_password(Some(USER)),
        Response::Opened { .. }
    ));
    assert!(renders_page_one(&mut engine));
    engine.send(&Request::Close);
    assert!(engine.wait().success());

    let mut text = Vec::new();
    log.seek(SeekFrom::Start(0)).unwrap();
    log.read_to_end(&mut text).unwrap();
    let text = String::from_utf8_lossy(&text);
    // The log has to exist and be verbose for this to prove anything.
    assert!(
        text.contains("opening with a password"),
        "the trace log is missing the password path:\n{text}"
    );
    assert!(text.contains("document opened"), "{text}");
    for secret in wrong.into_iter().chain([USER]) {
        assert!(
            !text.contains(secret),
            "the log contains {secret:?}:\n{text}"
        );
    }
    // Nor a recognisable part of one.
    for fragment in ["hunter2", "tr0ub4dor", "ünï"] {
        assert!(
            !text.contains(fragment),
            "the log contains {fragment:?}:\n{text}"
        );
    }
}
