//! Unlock: Remove the password from a PDF you can open.

use super::merge;
use crate::{Ctx, Error, Outcome, Result, doc};
use lopdf::{Document, Object};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Unlock has nothing to set: the password comes from the app (`Ctx::with_password`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Options {}

/// Whether the file is encrypted at all, and whether it opens without a password
/// (restricted but not locked). For the app, to decide whether to ask for a password.
pub fn status(path: &Path) -> Result<(bool, bool)> {
    let bytes = doc::read_file(path)?;
    match doc::load_bytes(&bytes, None) {
        Ok(d) => Ok((d.was_encrypted(), true)),
        Err(Error::PasswordRequired) => Ok((true, false)),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Opening protected files (shared by the organise tools)
// ---------------------------------------------------------------------------

/// The padding string of the standard security handler, revisions 2 to 4.
const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08, 0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80,
    0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    if key.is_empty() {
        return data.to_vec();
    }
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|byte| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            byte ^ s[s[i as usize].wrapping_add(s[j as usize]) as usize]
        })
        .collect()
}

/// For 40-bit and 128-bit encryption (revisions 2 to 4) the owner password does not
/// unlock the file directly: it unlocks the user password stored in `/O`. lopdf 0.45
/// accepts the owner password but then derives the key as if it were the user
/// password, and every string and stream comes out as noise without an error. So when
/// `password` is the owner password of such a file, recover the user password here
/// (ISO 32000-1, algorithm 7) and open the file with that.
pub(crate) fn user_password_behind_owner(bytes: &[u8], password: &str) -> Result<Option<String>> {
    if !bytes.windows(8).any(|w| w == b"/Encrypt") {
        return Ok(None);
    }
    let Ok(raw) = Document::load_mem(bytes) else { return Ok(None) };
    // A file that opens with the empty password comes back already decrypted.
    let Ok(encrypt) = raw.get_encrypted() else { return Ok(None) };
    let revision = encrypt.get(b"R").and_then(Object::as_i64).unwrap_or(0);
    if !(2..=4).contains(&revision) {
        return Ok(None);
    }
    if raw.authenticate_user_password(password).is_ok() || raw.authenticate_owner_password(password).is_err() {
        return Ok(None);
    }
    let Ok(owner_value) = encrypt.get(b"O").and_then(Object::as_str) else { return Ok(None) };
    if !password.is_ascii() {
        return Ok(None);
    }
    let key_len = if revision >= 3 { (encrypt.get(b"Length").and_then(Object::as_i64).unwrap_or(40) / 8).clamp(5, 16) as usize } else { 5 };

    let typed = password.as_bytes();
    let len = typed.len().min(32);
    let mut hasher = Md5::new();
    hasher.update(&typed[..len]);
    hasher.update(&PAD[..32 - len]);
    let mut hash: Vec<u8> = hasher.finalize().to_vec();
    if revision >= 3 {
        for _ in 0..50 {
            hash = Md5::digest(&hash).to_vec();
        }
    }
    let Some(key) = hash.get(..key_len) else { return Ok(None) };
    let mut padded = owner_value.to_vec();
    if revision >= 3 {
        for round in (0..=19u8).rev() {
            let round_key: Vec<u8> = key.iter().map(|b| b ^ round).collect();
            padded = rc4(&round_key, &padded);
        }
    } else {
        padded = rc4(key, &padded);
    }
    // The result is the user password followed by as much of the padding as fits in 32 bytes.
    let cut = (0..=padded.len().min(32)).find(|k| padded.get(*k..).map(|tail| PAD.starts_with(tail)).unwrap_or(false)).unwrap_or(padded.len());
    let user = padded.get(..cut).unwrap_or_default();
    if user.iter().all(|b| (0x20..=0x7E).contains(b)) {
        Ok(Some(String::from_utf8_lossy(user).into_owned()))
    } else {
        Err(Error::Unsupported(
            "This file was opened with its owner password, but its other password uses characters that cannot be recovered here. Open it with the password that is asked for when the file is opened.".into(),
        ))
    }
}

/// Load a PDF from bytes, with either of its passwords. Returns the document and the
/// password that actually decrypts it (pass that one on to `Renderer`/`TextReader`).
pub(crate) fn load_bytes(bytes: &[u8], password: Option<&str>) -> Result<(Document, Option<String>)> {
    if let Some(given) = password {
        if let Some(user) = user_password_behind_owner(bytes, given)? {
            let document = doc::load_bytes(bytes, Some(&user))?;
            return Ok((document, Some(user)));
        }
    }
    Ok((doc::load_bytes(bytes, password)?, password.map(str::to_string)))
}

/// `load_bytes` for a file on disk.
pub(crate) fn load(path: &Path, password: Option<&str>) -> Result<Document> {
    Ok(load_bytes(&doc::read_file(path)?, password)?.0)
}

fn open(bytes: &[u8], password: Option<&str>) -> Result<Document> {
    match load_bytes(bytes, password) {
        // A file that is only restricted opens with no password at all; do not turn a
        // mistyped owner password into a failure when it is not needed.
        Err(Error::WrongPassword) if password.is_some() => match doc::load_bytes(bytes, None) {
            Ok(d) => Ok(d),
            Err(_) => Err(Error::WrongPassword),
        },
        other => other.map(|(document, _)| document),
    }
}

pub fn run(inputs: &[PathBuf], out: &Path, _opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = merge::one_input(inputs, "Unlock")?;
    merge::guard_output(inputs, out)?;
    ctx.report(0.0, "Opening the file");
    let bytes = doc::read_file(input)?;
    let mut document = open(&bytes, ctx.password())?;
    let pages = doc::page_count(&document);
    ctx.check()?;

    if !document.was_encrypted() {
        // Nothing to remove: hand back the file exactly as it is.
        doc::write_file(out, &bytes)?;
        ctx.report(1.0, "Done");
        return Ok(Outcome::single(out.to_path_buf(), pages, bytes.len() as u64).note("This file was not password protected. A copy was saved unchanged."));
    }

    ctx.report(0.5, "Removing the protection");
    document.trailer.remove(b"Encrypt");
    document.objects.retain(|_, o| !matches!(o.type_name(), Ok(b"ObjStm") | Ok(b"XRef")));
    merge::prune(&mut document);
    let saved = doc::to_bytes(&mut document)?;

    // Make sure the copy really opens without a password before writing it.
    ctx.check()?;
    ctx.report(0.8, "Checking the result");
    let check = doc::load_bytes(&saved, None)?;
    if check.was_encrypted() || doc::page_count(&check) != pages {
        return Err(Error::other("The unlocked copy did not read back correctly, so nothing was written. The original file is untouched."));
    }
    doc::write_file(out, &saved)?;
    ctx.report(1.0, "Done");
    Ok(Outcome::single(out.to_path_buf(), pages, bytes.len() as u64).note("The password and the restrictions were removed."))
}
