//! Decryption through the object store, on files encrypted by qpdf.
//!
//! The fixtures in `tests/fixtures/encryption/` were made by `generate.py` there (qpdf 12.4 through
//! pikepdf), not by `cos`, so they check the Standard Security Handler against an independent
//! implementation: R2 (RC4 40), R3 (RC4 128), R4 (RC4 and AES-128 crypt filters), R6 (AES-256).
//! Every file holds the same content; see the script's header. Revision 5 has no fixture here
//! (qpdf will not write it); the corpus test in `store_corpus.rs` reads `pdfium-encrypted_hello_world_r5`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;

use vellora_cos::filter::decode_stream;
use vellora_cos::{
    CryptMethod, EncryptionError, Error, Limits, ObjRef, Object, ObjectKind, ObjectStore,
    PasswordRole,
};

const OWNER: &[u8] = b"owner-pw";
const USER: &[u8] = b"user-pw";

/// name, revision, key bits, stream method, string method, `/EncryptMetadata`, user password set
const FIXTURES: &[(&str, u32, u32, CryptMethod, CryptMethod, bool, bool)] = &[
    (
        "r2-rc4-40",
        2,
        40,
        CryptMethod::Rc4,
        CryptMethod::Rc4,
        true,
        false,
    ),
    (
        "r3-rc4-128",
        3,
        128,
        CryptMethod::Rc4,
        CryptMethod::Rc4,
        true,
        false,
    ),
    (
        "r4-rc4-128",
        4,
        128,
        CryptMethod::Rc4,
        CryptMethod::Rc4,
        false,
        false,
    ),
    (
        "r4-aes-128",
        4,
        128,
        CryptMethod::AesV2,
        CryptMethod::AesV2,
        true,
        false,
    ),
    (
        "r4-aes-128-no-metadata",
        4,
        128,
        CryptMethod::AesV2,
        CryptMethod::AesV2,
        false,
        false,
    ),
    (
        "r6-aes-256",
        6,
        256,
        CryptMethod::AesV3,
        CryptMethod::AesV3,
        true,
        false,
    ),
    (
        "r2-rc4-40-user-password",
        2,
        40,
        CryptMethod::Rc4,
        CryptMethod::Rc4,
        true,
        true,
    ),
    (
        "r3-rc4-128-user-password",
        3,
        128,
        CryptMethod::Rc4,
        CryptMethod::Rc4,
        true,
        true,
    ),
    (
        "r4-aes-128-user-password",
        4,
        128,
        CryptMethod::AesV2,
        CryptMethod::AesV2,
        true,
        true,
    ),
    (
        "r6-aes-256-user-password",
        6,
        256,
        CryptMethod::AesV3,
        CryptMethod::AesV3,
        true,
        true,
    ),
];

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/encryption")
        .join(format!("{name}.pdf"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn open(data: &[u8]) -> ObjectStore<'_> {
    ObjectStore::open(data, Limits::default()).unwrap()
}

fn r(num: u32) -> ObjRef {
    ObjRef::new(num, 0)
}

fn string_of(dict: &Object<'_>, key: &[u8]) -> Vec<u8> {
    match &dict.as_dict().unwrap().get(key).unwrap().kind {
        ObjectKind::String(s) => s.to_vec(),
        other => panic!("/{} is {other:?}", String::from_utf8_lossy(key)),
    }
}

fn reference_of(dict: &Object<'_>, key: &[u8]) -> ObjRef {
    match dict.as_dict().unwrap().get(key).unwrap().kind {
        ObjectKind::Ref(reference) => reference,
        _ => panic!("/{} is not a reference", String::from_utf8_lossy(key)),
    }
}

/// What the fixtures contain, read through the store.
struct Content {
    title: Vec<u8>,
    marker: Vec<u8>,
    reason: Vec<u8>,
    signature: Vec<u8>,
    page_text: Vec<u8>,
    metadata: Vec<u8>,
}

fn read_content(store: &ObjectStore<'_>) -> Content {
    let info = store.info().unwrap().unwrap();
    let root = store.root().unwrap().unwrap();
    let sig = store.resolve(reference_of(&root, b"SigTest")).unwrap();

    let page = store.pages().next().unwrap().unwrap();
    let contents = reference_of(&page.object, b"Contents");
    let stream = store.resolve(contents).unwrap();
    let raw = store.stream_decrypted(contents).unwrap().unwrap();
    let limits = Limits::default();
    let page_text = decode_stream(stream.as_dict().unwrap(), &raw, &limits, None, None)
        .unwrap()
        .data;

    let metadata = reference_of(&root, b"Metadata");
    let stream = store.resolve(metadata).unwrap();
    let raw = store.stream_decrypted(metadata).unwrap().unwrap();
    let metadata = decode_stream(stream.as_dict().unwrap(), &raw, &limits, None, None)
        .unwrap()
        .data;
    Content {
        title: string_of(&info, b"Title"),
        marker: string_of(&root, b"Marker"),
        reason: string_of(&sig, b"Reason"),
        signature: string_of(&sig, b"Contents"),
        page_text,
        metadata,
    }
}

fn assert_content(store: &ObjectStore<'_>, name: &str) {
    let c = read_content(store);
    assert_eq!(c.title, b"Secret title", "{name}: /Info /Title");
    assert_eq!(c.marker, b"Catalog marker", "{name}: catalog string");
    assert_eq!(
        c.reason, b"because",
        "{name}: string in a /Type /Sig dictionary"
    );
    assert_eq!(
        c.signature, b"SIGNATURE-BYTES-0123456789",
        "{name}: signature /Contents"
    );
    assert!(
        String::from_utf8_lossy(&c.page_text).contains("(Hello, encrypted world) Tj"),
        "{name}: content stream is {:?}",
        String::from_utf8_lossy(&c.page_text)
    );
    assert!(
        String::from_utf8_lossy(&c.metadata).contains("VELLORA-METADATA"),
        "{name}: metadata stream"
    );
}

#[test]
fn the_empty_user_password_opens_files_without_one() {
    for &(name, revision, bits, stream, string, metadata, user_password) in FIXTURES {
        if user_password {
            continue;
        }
        let data = fixture(name);
        let store = open(&data);
        assert!(!store.is_locked(), "{name}");
        assert_eq!(store.password_role(), Some(PasswordRole::User), "{name}");
        let info = store.encryption_info().unwrap();
        assert_eq!(info.revision, revision, "{name}");
        assert_eq!(info.key_bits, bits, "{name}");
        assert_eq!(info.stream_method, stream, "{name}");
        assert_eq!(info.string_method, string, "{name}");
        assert_eq!(info.encrypt_metadata, metadata, "{name}");
        assert_content(&store, name);
        assert!(
            store.repaired().is_empty(),
            "{name}: {:?}",
            store.repaired()
        );
    }
}

#[test]
fn files_with_a_user_password_stay_locked_until_it_is_given() {
    for &(name, .., user_password) in FIXTURES {
        if !user_password {
            continue;
        }
        let data = fixture(name);
        let store = open(&data);
        assert!(store.is_locked(), "{name}");
        assert_eq!(store.password_role(), None, "{name}");
        // Nothing is readable, so ciphertext is never mistaken for content.
        for result in [
            store.resolve(r(1)).map(drop),
            store.root().map(drop),
            store.info().map(drop),
            store.stream_decrypted(r(1)).map(drop),
        ] {
            assert!(
                matches!(
                    result,
                    Err(Error::Encryption {
                        kind: EncryptionError::PasswordRequired
                    })
                ),
                "{name}: {result:?}"
            );
        }
        assert!(store.pages().next().unwrap().is_err(), "{name}");
        // The dictionary itself can be inspected.
        assert!(store.encrypt().unwrap().is_some(), "{name}");
        assert!(store.encryption_info().is_some(), "{name}");

        assert!(
            matches!(
                store.authenticate(b"wrong"),
                Err(Error::Encryption {
                    kind: EncryptionError::IncorrectPassword
                })
            ),
            "{name}"
        );
        assert!(
            store.is_locked(),
            "{name}: a wrong password changes nothing"
        );
        assert!(store.authenticate(b"").is_err(), "{name}");

        assert_eq!(
            store.authenticate(USER).unwrap(),
            PasswordRole::User,
            "{name}"
        );
        assert!(!store.is_locked(), "{name}");
        assert_eq!(store.password_role(), Some(PasswordRole::User), "{name}");
        assert_content(&store, name);
    }
}

#[test]
fn the_owner_password_opens_every_file() {
    for &(name, ..) in FIXTURES {
        let data = fixture(name);
        let store = open(&data);
        assert_eq!(
            store.authenticate(OWNER).unwrap(),
            PasswordRole::Owner,
            "{name}"
        );
        assert_eq!(store.password_role(), Some(PasswordRole::Owner), "{name}");
        assert_content(&store, name);
    }
}

#[test]
fn a_wrong_password_is_a_typed_error_and_leaves_an_open_store_readable() {
    for &(name, .., user_password) in FIXTURES {
        let data = fixture(name);
        let store = open(&data);
        for wrong in [&b"wrong"[..], b"user-p", b"owner-pw ", &[0xFF; 300]] {
            assert!(
                matches!(
                    store.authenticate(wrong),
                    Err(Error::Encryption {
                        kind: EncryptionError::IncorrectPassword
                    })
                ),
                "{name}: {wrong:?}"
            );
        }
        if !user_password {
            // Opened with the empty password at the start; still readable.
            assert!(!store.is_locked(), "{name}");
            assert_content(&store, name);
        }
    }
}

#[test]
fn passwords_are_cut_to_the_length_the_standard_allows() {
    // Revisions 2-4 use the first 32 bytes, 5-6 the first 127: a longer password with the same
    // beginning must not be accepted by accident, and the real one still works after trying it.
    let data = fixture("r6-aes-256-user-password");
    let store = open(&data);
    let mut long = USER.to_vec();
    long.extend([b'x'; 200]);
    assert!(store.authenticate(&long).is_err());
    assert!(store.authenticate(USER).is_ok());
}

#[test]
fn the_encrypt_dictionary_is_readable_and_never_decrypted() {
    for &(name, revision, ..) in FIXTURES {
        let data = fixture(name);
        let store = open(&data);
        let dict = store.encrypt().unwrap().unwrap();
        let entries = dict.as_dict().unwrap();
        assert!(matches!(
            &entries.get(b"Filter").unwrap().kind,
            ObjectKind::Name(n) if n.as_ref() == b"Standard"
        ));
        assert_eq!(
            entries.get(b"R").unwrap().as_integer(),
            Some(i64::from(revision)),
            "{name}"
        );
        // /O and /U are stored as they are: 32 bytes, or 48 from revision 5.
        let expected = if revision >= 5 { 48 } else { 32 };
        assert_eq!(string_of(&dict, b"O").len(), expected, "{name}");
        assert_eq!(string_of(&dict, b"U").len(), expected, "{name}");
    }
}

#[test]
fn metadata_stays_plain_when_the_dictionary_says_so() {
    let data = fixture("r4-aes-128-no-metadata");
    let store = open(&data);
    let root = store.root().unwrap().unwrap();
    let reference = reference_of(&root, b"Metadata");
    let stream = store.resolve(reference).unwrap();
    let ObjectKind::Stream(stream) = &stream.kind else {
        panic!("not a stream");
    };
    let raw = store.stream_raw(stream).unwrap();
    assert!(String::from_utf8_lossy(raw).contains("VELLORA-METADATA"));
    let decrypted = store.stream_decrypted(reference).unwrap().unwrap();
    assert!(
        matches!(decrypted, Cow::Borrowed(_)),
        "nothing to decrypt, nothing copied"
    );
    assert_eq!(&*decrypted, raw);

    // Encrypted metadata, in contrast, is not readable as it is stored.
    let data = fixture("r4-aes-128");
    let store = open(&data);
    let root = store.root().unwrap().unwrap();
    let reference = reference_of(&root, b"Metadata");
    let stream = store.resolve(reference).unwrap();
    let ObjectKind::Stream(stream) = &stream.kind else {
        panic!("not a stream");
    };
    assert!(
        !String::from_utf8_lossy(store.stream_raw(stream).unwrap()).contains("VELLORA-METADATA")
    );
}

#[test]
fn encrypted_object_streams_are_decrypted_as_a_whole() {
    for name in ["r4-aes-128", "r6-aes-256"] {
        let data = fixture(name);
        assert!(
            data.windows(8).any(|w| w == b"/ObjStm "),
            "{name}: the fixture is meant to use object streams"
        );
        let store = open(&data);
        // /Info is in an object stream: reading it needs the container decrypted, and its
        // strings must not be decrypted a second time.
        let info = store.info().unwrap().unwrap();
        assert_eq!(string_of(&info, b"Title"), b"Secret title", "{name}");
    }
}

#[test]
fn the_same_store_gives_the_same_answers_before_and_after_re_authenticating() {
    let data = fixture("r4-aes-128");
    let store = open(&data);
    let before = read_content(&store).title;
    assert_eq!(store.authenticate(OWNER).unwrap(), PasswordRole::Owner);
    assert_eq!(read_content(&store).title, before);
    assert_eq!(store.authenticate(b"").unwrap(), PasswordRole::User);
    assert_eq!(read_content(&store).title, before);
}

#[test]
fn an_unencrypted_store_accepts_any_password() {
    let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n";
    let store = open(data);
    assert!(!store.is_locked());
    assert!(store.encryption_info().is_none());
    assert_eq!(store.authenticate(b"anything").unwrap(), PasswordRole::User);
    assert_eq!(store.password_role(), None);
}

/// A file with a one-object body and a correct classic table; `trailer_extra` goes in the trailer.
fn minimal_pdf(objects: &[(u32, &str)], trailer_extra: &str) -> Vec<u8> {
    let mut data = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (number, body) in objects {
        offsets.push((*number, data.len()));
        data.extend(format!("{number} 0 obj\n{body}\nendobj\n").bytes());
    }
    let size = offsets.iter().map(|o| o.0).max().unwrap() + 1;
    let at = data.len();
    let mut table = format!("xref\n0 {size}\n0000000000 65535 f \n");
    for number in 1..size {
        let offset = offsets.iter().find(|o| o.0 == number).unwrap().1;
        writeln!(table, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        table,
        "trailer\n<< /Size {size} /Root 1 0 R {trailer_extra} >>\nstartxref\n{at}\n%%EOF\n"
    )
    .unwrap();
    data.extend(table.bytes());
    data
}

fn open_error(data: &[u8]) -> EncryptionError {
    match ObjectStore::open(data, Limits::default()) {
        Err(Error::Encryption { kind }) => kind,
        other => panic!("expected an encryption error, got {other:?}"),
    }
}

#[test]
fn unsupported_or_broken_encryption_fails_to_open_with_a_typed_error() {
    let o = "(".to_owned() + &"o".repeat(32) + ")";
    let cases: Vec<(String, EncryptionError)> = vec![
        (
            "<< /Filter /Adobe.PubSec /SubFilter /adbe.pkcs7.s5 /V 4 >>".into(),
            EncryptionError::UnsupportedHandler,
        ),
        (
            format!("<< /Filter /Standard /V 3 /R 3 /O {o} /U {o} /P -4 >>"),
            EncryptionError::UnsupportedVersion { version: 3 },
        ),
        (
            format!("<< /Filter /Standard /V 2 /R 9 /O {o} /U {o} /P -4 >>"),
            EncryptionError::UnsupportedRevision { revision: 9 },
        ),
        (
            format!(
                "<< /Filter /Standard /V 4 /R 4 /O {o} /U {o} /P -4 \
                 /CF << /StdCF << /CFM /AESV2 >> >> /StmF /Nope /StrF /StdCF >>"
            ),
            EncryptionError::UnknownCryptFilter,
        ),
    ];
    for (dict, expected) in cases {
        let data = minimal_pdf(
            &[(1, "<< /Type /Catalog >>"), (2, &dict)],
            "/Encrypt 2 0 R /ID [(a) (a)]",
        );
        assert_eq!(open_error(&data), expected, "{dict}");
    }
    let data = minimal_pdf(&[(1, "<< /Type /Catalog >>")], "/Encrypt 5 /ID [(a) (a)]");
    assert!(matches!(open_error(&data), EncryptionError::Malformed(_)));
    // The dictionary may also be direct in the trailer.
    let data = minimal_pdf(
        &[(1, "<< /Type /Catalog >>")],
        "/Encrypt << /Filter /Adobe.PubSec >>",
    );
    assert_eq!(open_error(&data), EncryptionError::UnsupportedHandler);
}

#[test]
fn damaged_encrypted_files_never_panic() {
    // Flip bytes of a few fixtures in turn: the store must answer with data or a typed error. (A
    // flipped byte can also make the password check fail, which is a typed error too.) Every byte
    // for the cheap revisions; every 40th for revision 6, whose password check hashes for
    // hundreds of rounds and is run twice per file here.
    for (name, step) in [("r3-rc4-128", 1), ("r4-aes-128", 1), ("r6-aes-256", 40)] {
        let original = fixture(name);
        for at in (0..original.len()).step_by(step) {
            let mut data = original.clone();
            data[at] ^= 0x5A;
            let Ok(store) = ObjectStore::open(&data, Limits::default()) else {
                continue;
            };
            let _ = store.authenticate(OWNER);
            let _ = store.info();
            let _ = store.root();
            for page in store.pages().take(4) {
                let _ = page;
            }
            let _ = store.stream_decrypted(r(1));
            let _ = store.stream_decrypted(r(5));
            let _ = store.stream_decrypted(r(6));
        }
    }
}
