//! JS string value type: UTF-16 semantics with lone-surrogate support.
//!
//! JS strings are sequences of UTF-16 code units and may contain *lone
//! surrogates* (ill-formed UTF-16), which a Rust `String`/`str` cannot hold.
//! [`JsString`] keeps the common well-formed case as a cheap UTF-8 `Arc<str>`
//! (so the vast majority of the interpreter reads it as `&str` via `Deref` with
//! zero overhead and unchanged behavior), and only falls back to storing raw
//! `u16` code units when a string actually contains a lone surrogate (produced
//! by e.g. `"😀".charAt(0)` or `String.fromCharCode(0xD83D)`).
//!
//! Length and indexing are UTF-16-based (see [`JsString::len_utf16`] /
//! [`JsString::units`]); textual `&str` access via `Deref` is lossy only for the
//! rare lone-surrogate case (which has no valid UTF-8 form).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum JsString {
    /// Well-formed (no lone surrogates), stored as UTF-8 for cheap `&str` access.
    Valid(Arc<str>),
    /// Contains lone surrogate(s): the exact UTF-16 units, plus a lossy UTF-8
    /// rendering so `Deref<str>`/`Display` still work for textual consumers.
    Wtf { units: Arc<[u16]>, lossy: Arc<str> },
}

impl JsString {
    /// The textual content as `&str` (lossy — replacement chars — only for the
    /// rare lone-surrogate case, which has no valid UTF-8 representation).
    pub fn as_str(&self) -> &str {
        match self {
            JsString::Valid(s) => s,
            JsString::Wtf { lossy, .. } => lossy,
        }
    }

    /// The UTF-16 code units (what JS `length`/`charCodeAt`/indexing operate on).
    pub fn units(&self) -> Cow<'_, [u16]> {
        match self {
            JsString::Valid(s) => Cow::Owned(s.encode_utf16().collect()),
            JsString::Wtf { units, .. } => Cow::Borrowed(units),
        }
    }

    /// Number of UTF-16 code units — JS `String.prototype.length`.
    pub fn len_utf16(&self) -> usize {
        match self {
            JsString::Valid(s) => s.encode_utf16().count(),
            JsString::Wtf { units, .. } => units.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            JsString::Valid(s) => s.is_empty(),
            JsString::Wtf { units, .. } => units.is_empty(),
        }
    }

    /// Build a `JsString` from UTF-16 code units, choosing the well-formed
    /// (`Valid`) representation when there are no lone surrogates and the
    /// lone-surrogate (`Wtf`) representation otherwise.
    pub fn from_units(units: &[u16]) -> JsString {
        match String::from_utf16(units) {
            Ok(s) => JsString::Valid(Arc::from(s.as_str())),
            Err(_) => {
                let lossy = String::from_utf16_lossy(units);
                JsString::Wtf {
                    units: Arc::from(units),
                    lossy: Arc::from(lossy.as_str()),
                }
            }
        }
    }
}

impl Deref for JsString {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for JsString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for JsString {
    fn from(s: &str) -> Self {
        JsString::Valid(Arc::from(s))
    }
}

impl From<String> for JsString {
    fn from(s: String) -> Self {
        JsString::Valid(Arc::from(s.as_str()))
    }
}

impl From<&String> for JsString {
    fn from(s: &String) -> Self {
        JsString::Valid(Arc::from(s.as_str()))
    }
}

impl From<Arc<str>> for JsString {
    fn from(s: Arc<str>) -> Self {
        JsString::Valid(s)
    }
}

impl From<char> for JsString {
    fn from(c: char) -> Self {
        JsString::Valid(Arc::from(c.to_string().as_str()))
    }
}

impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Equality is by UTF-16 code units. Two `Valid` strings compare by their text;
/// a `Valid` and a `Wtf` are never equal (a `Wtf` contains a lone surrogate that
/// no valid string has); two `Wtf` strings compare by their units.
impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JsString::Valid(a), JsString::Valid(b)) => a == b,
            (JsString::Wtf { units: a, .. }, JsString::Wtf { units: b, .. }) => a == b,
            _ => false,
        }
    }
}
impl Eq for JsString {}

impl PartialEq<str> for JsString {
    fn eq(&self, other: &str) -> bool {
        matches!(self, JsString::Valid(s) if s.as_ref() == other)
    }
}
impl PartialEq<&str> for JsString {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl Ord for JsString {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Lexicographic by UTF-16 code units (JS string comparison order).
        self.units().cmp(&other.units())
    }
}
impl PartialOrd for JsString {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for JsString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Equal values hash equal: `Valid` hashes its text; `Wtf` hashes units.
        // (They are never equal to each other, so distinct hashing is fine.)
        match self {
            JsString::Valid(s) => {
                0u8.hash(state);
                s.hash(state);
            }
            JsString::Wtf { units, .. } => {
                1u8.hash(state);
                units.hash(state);
            }
        }
    }
}

impl Default for JsString {
    fn default() -> Self {
        JsString::Valid(Arc::from(""))
    }
}

/// On-the-wire representation. A fixed, enum-tagged shape (NOT
/// `deserialize_any`) so it round-trips through the non-self-describing
/// snapshot format (postcard). `Valid` is the overwhelmingly common case;
/// `Wtf` carries raw UTF-16 units for the rare lone-surrogate string.
#[derive(Serialize, Deserialize)]
enum JsStringRepr {
    Valid(String),
    Wtf(Vec<u16>),
}

impl Serialize for JsString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let repr = match self {
            JsString::Valid(s) => JsStringRepr::Valid(s.to_string()),
            JsString::Wtf { units, .. } => JsStringRepr::Wtf(units.to_vec()),
        };
        repr.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JsString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match JsStringRepr::deserialize(deserializer)? {
            JsStringRepr::Valid(s) => Ok(JsString::Valid(Arc::from(s.as_str()))),
            JsStringRepr::Wtf(units) => Ok(JsString::from_units(&units)),
        }
    }
}
