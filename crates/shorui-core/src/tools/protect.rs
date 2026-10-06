//! Protect: Encrypt a PDF with a password.

use super::merge;
use crate::ctx::file_size;
use crate::{Ctx, Error, Outcome, Result, doc};
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::xref::XrefType;
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Encryption {
    /// AES with a 256-bit key (PDF 2.0). Opens in any reader from the last decade.
    #[default]
    #[serde(rename = "aes-256")]
    Aes256,
    /// AES with a 128-bit key, for very old readers.
    #[serde(rename = "aes-128")]
    Aes128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Needed to open the file. May be empty when the aim is only to restrict what
    /// readers may do with it.
    pub user_password: String,
    /// Lifts the restrictions. Defaults to the user password.
    pub owner_password: String,
    pub encryption: Encryption,
    pub allow_print: bool,
    pub allow_copy: bool,
    pub allow_modify: bool,
    pub allow_annotate: bool,
    pub allow_forms: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            user_password: String::new(),
            owner_password: String::new(),
            encryption: Encryption::Aes256,
            allow_print: true,
            allow_copy: true,
            allow_modify: true,
            allow_annotate: true,
            allow_forms: true,
        }
    }
}

pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::other(format!("This computer could not supply random numbers ({e}), so nothing was written.")))?;
    Ok(bytes)
}

/// A fresh file identifier for the trailer.
pub(crate) fn new_file_id() -> Result<Object> {
    let id = random_bytes::<16>()?;
    Ok(Object::Array(vec![Object::String(id.to_vec(), StringFormat::Hexadecimal), Object::String(id.to_vec(), StringFormat::Hexadecimal)]))
}

fn has_file_id(doc: &Document) -> bool {
    match doc.trailer.get(b"ID") {
        Ok(Object::Array(items)) => items.len() == 2 && items.iter().all(|o| matches!(o, Object::String(s, _) if !s.is_empty())),
        _ => false,
    }
}

fn permissions(opts: &Options) -> Permissions {
    let mut p = Permissions::COPYABLE_FOR_ACCESSIBILITY;
    if opts.allow_print {
        p |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY;
    }
    if opts.allow_copy {
        p |= Permissions::COPYABLE;
    }
    if opts.allow_modify {
        p |= Permissions::MODIFIABLE | Permissions::ASSEMBLABLE;
    }
    if opts.allow_annotate {
        p |= Permissions::ANNOTABLE;
    }
    if opts.allow_forms {
        p |= Permissions::FILLABLE;
    }
    p
}

/// Switch every string in an object to hexadecimal notation.
fn hex_strings(object: &mut Object) {
    match object {
        Object::String(_, format) => *format = StringFormat::Hexadecimal,
        Object::Array(items) => items.iter_mut().for_each(hex_strings),
        Object::Dictionary(dict) => dict.iter_mut().for_each(|(_, v)| hex_strings(v)),
        Object::Stream(stream) => stream.dict.iter_mut().for_each(|(_, v)| hex_strings(v)),
        _ => {}
    }
}

fn password_error(e: lopdf::Error) -> Error {
    Error::invalid(format!("That password cannot be used ({e}). Try one made of letters, digits and common symbols."))
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = merge::one_input(inputs, "Protect")?;
    merge::guard_output(inputs, out)?;
    if opts.user_password.is_empty() && opts.owner_password.is_empty() {
        return Err(Error::invalid("Enter a password. Without one the file cannot be protected."));
    }
    let user = opts.user_password.as_str();
    let owner = if opts.owner_password.is_empty() { user } else { opts.owner_password.as_str() };
    if opts.encryption == Encryption::Aes128 {
        // 128-bit encryption takes passwords in a single-byte encoding and reads at most 32 bytes.
        for password in [user, owner] {
            if password.chars().any(|c| !(' '..='~').contains(&c)) {
                return Err(Error::invalid("With 128-bit encryption a password may only use plain letters, digits and symbols. Change the password or choose AES-256."));
            }
            if password.len() > 32 {
                return Err(Error::invalid("With 128-bit encryption a password may be at most 32 characters long. Shorten it or choose AES-256."));
            }
        }
    } else if user.len() > 127 || owner.len() > 127 {
        return Err(Error::invalid("A password may be at most 127 bytes long."));
    }

    ctx.report(0.0, "Reading the file");
    let mut document = super::unlock::load(input, ctx.password())?;
    let pages = doc::page_count(&document);

    // Everything that changes the content has to happen before encryption: afterwards
    // every string and stream is ciphertext.
    ctx.check()?;
    ctx.report(0.2, "Preparing");
    doc::set_info(&mut document, "Producer", doc::PRODUCER)?;
    document.objects.retain(|_, o| !matches!(o.type_name(), Ok(b"ObjStm") | Ok(b"XRef")));
    merge::prune(&mut document);
    document.compress();
    document.reference_table.cross_reference_type = XrefType::CrossReferenceTable;
    if !has_file_id(&document) {
        document.trailer.set("ID", new_file_id()?);
    }
    let minimum = if opts.encryption == Encryption::Aes256 { "1.7" } else { "1.6" };
    if document.version.as_str() < minimum {
        document.version = minimum.to_string();
    }
    if opts.encryption == Encryption::Aes256 {
        // 256-bit AES is an extension to PDF 1.7 (and standard in 2.0); say so for older readers.
        document.catalog_mut()?.set("Extensions", dictionary! { "ADBE" => dictionary! { "BaseVersion" => "1.7", "ExtensionLevel" => 8 } });
    }

    ctx.check()?;
    ctx.report(0.4, "Encrypting");
    let permissions = permissions(opts);
    let key = random_bytes::<32>()?;
    let state = match opts.encryption {
        Encryption::Aes256 => {
            let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
            EncryptionState::try_from(EncryptionVersion::V5 {
                encrypt_metadata: true,
                crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
                file_encryption_key: &key,
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: owner,
                user_password: user,
                permissions,
            })
        }
        Encryption::Aes128 => {
            let filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
            EncryptionState::try_from(EncryptionVersion::V4 {
                document: &document,
                encrypt_metadata: true,
                crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: owner,
                user_password: user,
                permissions,
            })
        }
    }
    .map_err(password_error)?;
    document.encrypt(&state).map_err(|e| Error::other(format!("The file could not be encrypted: {e}")))?;

    // lopdf leaves the key length out of a 256-bit encryption dictionary. Readers built on
    // Poppler then assume 40 bits, accept the password and decrypt everything to noise.
    // Spell the lengths out the way other writers do.
    let (bits, bytes_per_key) = if opts.encryption == Encryption::Aes256 { (256, 32) } else { (128, 16) };
    if let Ok(encrypt_id) = document.trailer.get(b"Encrypt").and_then(Object::as_reference) {
        let dict = document.get_dictionary_mut(encrypt_id)?;
        dict.set("Length", bits);
        if let Ok(Object::Dictionary(filters)) = dict.get_mut(b"CF") {
            if let Ok(Object::Dictionary(filter)) = filters.get_mut(b"StdCF") {
                filter.set("AuthEvent", "DocOpen");
                filter.set("Length", bytes_per_key);
            }
        }
    }

    // Ciphertext is arbitrary bytes. Written as literal strings it depends on every reader
    // undoing the escapes identically (hayro, for one, folds two raw line feeds into one,
    // which corrupts a key or a value now and then). Hexadecimal strings have no escapes.
    for object in document.objects.values_mut() {
        hex_strings(object);
    }
    let mut trailer_id = document.trailer.get(b"ID").ok().cloned();
    if let Some(id) = trailer_id.as_mut() {
        hex_strings(id);
        document.trailer.set("ID", id.clone());
    }

    let mut bytes = Vec::new();
    document.save_to(&mut bytes).map_err(|e| Error::other(format!("Could not build the PDF: {e}")))?;

    // Open the result again before writing it: a protected file nobody can open would be worse than none.
    ctx.check()?;
    ctx.report(0.8, "Checking the result");
    for password in [user, owner] {
        let (reopened, _) = super::unlock::load_bytes(&bytes, Some(password))
            .map_err(|e| Error::other(format!("The encrypted file did not open again with its password ({e}), so nothing was written.")))?;
        // The producer entry was a known string before encryption: it must decrypt to the same.
        let producer = merge::info_entry(&reopened, "Producer").and_then(|o| o.as_str().ok().map(<[u8]>::to_vec));
        if doc::page_count(&reopened) != pages || producer.as_deref() != Some(doc::PRODUCER.as_bytes()) {
            return Err(Error::other("The encrypted file did not read back correctly, so nothing was written."));
        }
    }
    doc::write_file(out, &bytes)?;
    ctx.report(1.0, "Done");

    let mut outcome = Outcome::single(out.to_path_buf(), pages, file_size(input));
    outcome.notes.push(match opts.encryption {
        Encryption::Aes256 => "Encrypted with AES-256.".to_string(),
        Encryption::Aes128 => "Encrypted with AES-128.".to_string(),
    });
    let restricted = !(opts.allow_print && opts.allow_copy && opts.allow_modify && opts.allow_annotate && opts.allow_forms);
    if user.is_empty() {
        outcome.notes.push("The file opens without a password. The restrictions depend on the reader respecting them; they are not a lock.".into());
    } else if restricted && owner == user {
        outcome.notes.push("One password both opens the file and lifts the restrictions. Set a separate owner password to keep the restrictions for people who can open it.".into());
    } else if restricted {
        outcome.notes.push("The restrictions depend on the reader respecting them; anyone who can open the file can read all of it.".into());
    }
    Ok(outcome)
}
