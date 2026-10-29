//! Shared transcript storage. Snapshots share both the item index and individual
//! JSON payloads; changing one row does not copy unrelated tool output.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::{Borrow, BorrowMut},
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A lossless JSON item shared between provider state and displayed snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TranscriptItem(Arc<Value>);
impl TranscriptItem {
    pub fn into_value(self) -> Value {
        Arc::try_unwrap(self.0).unwrap_or_else(|value| value.as_ref().clone())
    }
}
impl std::fmt::Display for TranscriptItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.as_ref(), formatter)
    }
}
impl From<Value> for TranscriptItem {
    fn from(value: Value) -> Self {
        Self(Arc::new(value))
    }
}
impl Deref for TranscriptItem {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl DerefMut for TranscriptItem {
    fn deref_mut(&mut self) -> &mut Value {
        Arc::make_mut(&mut self.0)
    }
}
impl AsRef<Value> for TranscriptItem {
    fn as_ref(&self) -> &Value {
        &self.0
    }
}
impl Borrow<Value> for TranscriptItem {
    fn borrow(&self) -> &Value {
        &self.0
    }
}
impl BorrowMut<Value> for TranscriptItem {
    fn borrow_mut(&mut self) -> &mut Value {
        self
    }
}
impl PartialEq for TranscriptItem {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}
impl Eq for TranscriptItem {}
impl PartialEq<Value> for TranscriptItem {
    fn eq(&self, other: &Value) -> bool {
        self.as_ref() == other
    }
}

/// JSON-compatible transcript items with copy-on-write snapshot ownership.
///
/// The revision is process-local and absent from recovery files. Every mutable
/// collection borrow obtains a new revision, including equal-length edits.
/// Forking the index copies only item references; JSON is copied lazily for the
/// row actually changed, including when an iter_mut search visits older rows.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TranscriptItems {
    values: Arc<Vec<TranscriptItem>>,
    #[serde(skip, default = "next_revision")]
    revision: u64,
}
impl TranscriptItems {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn push(&mut self, item: impl Into<TranscriptItem>) {
        self.deref_mut().push(item.into());
    }
    pub fn extend<I>(&mut self, items: I)
    where
        I: IntoIterator,
        I::Item: Into<TranscriptItem>,
    {
        self.deref_mut().extend(items.into_iter().map(Into::into));
    }
    /// Clearing a conversation never needs a copy of the discarded payload.
    pub fn clear(&mut self) {
        self.values = Arc::new(Vec::new());
        self.revision = next_revision();
    }
}
impl Default for TranscriptItems {
    fn default() -> Self {
        Self::from(Vec::<Value>::new())
    }
}
impl From<Vec<Value>> for TranscriptItems {
    fn from(values: Vec<Value>) -> Self {
        values.into_iter().map(TranscriptItem::from).collect()
    }
}
impl From<TranscriptItems> for Vec<Value> {
    fn from(items: TranscriptItems) -> Self {
        Arc::try_unwrap(items.values)
            .unwrap_or_else(|values| values.as_ref().clone())
            .into_iter()
            .map(TranscriptItem::into_value)
            .collect()
    }
}
impl Deref for TranscriptItems {
    type Target = Vec<TranscriptItem>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl DerefMut for TranscriptItems {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.revision = next_revision();
        Arc::make_mut(&mut self.values)
    }
}
impl PartialEq for TranscriptItems {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.values, &other.values) || self.values == other.values
    }
}
impl Eq for TranscriptItems {}
impl PartialEq<Vec<Value>> for TranscriptItems {
    fn eq(&self, other: &Vec<Value>) -> bool {
        self == other.as_slice()
    }
}
impl PartialEq<[Value]> for TranscriptItems {
    fn eq(&self, other: &[Value]) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(item, value)| item == value)
    }
}
impl PartialEq<Vec<TranscriptItem>> for TranscriptItems {
    fn eq(&self, other: &Vec<TranscriptItem>) -> bool {
        self.values.as_ref() == other
    }
}
impl PartialEq<[TranscriptItem]> for TranscriptItems {
    fn eq(&self, other: &[TranscriptItem]) -> bool {
        self.values.as_slice() == other
    }
}
impl FromIterator<Value> for TranscriptItems {
    fn from_iter<T: IntoIterator<Item = Value>>(items: T) -> Self {
        Vec::from_iter(items).into()
    }
}
impl FromIterator<TranscriptItem> for TranscriptItems {
    fn from_iter<T: IntoIterator<Item = TranscriptItem>>(items: T) -> Self {
        Self {
            values: Arc::new(Vec::from_iter(items)),
            revision: next_revision(),
        }
    }
}
impl IntoIterator for TranscriptItems {
    type Item = Value;
    type IntoIter = std::vec::IntoIter<Value>;
    fn into_iter(self) -> Self::IntoIter {
        Vec::from(self).into_iter()
    }
}
impl<'a> IntoIterator for &'a TranscriptItems {
    type Item = &'a TranscriptItem;
    type IntoIter = std::slice::Iter<'a, TranscriptItem>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}
impl<'a> IntoIterator for &'a mut TranscriptItems {
    type Item = &'a mut TranscriptItem;
    type IntoIter = std::slice::IterMut<'a, TranscriptItem>;
    fn into_iter(self) -> Self::IntoIter {
        self.deref_mut().iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn edits_share_untouched_payloads_and_preserve_previous_snapshots() {
        let original: TranscriptItems = vec![json!({"id":"image", "result":{"content":[{"type":"image","data":"large image payload".repeat(10000)}]}}), json!({"id":"prose", "text":"before"})].into();
        let serialized = serde_json::to_value(&original).unwrap();
        let mut edited = original.clone();
        assert!(Arc::ptr_eq(&original.values, &edited.values));
        assert_eq!(original.revision(), edited.revision());
        edited.push(json!({"id":"append", "text":"new row"}));
        assert!(!Arc::ptr_eq(&original.values, &edited.values));
        assert!(Arc::ptr_eq(&original[0].0, &edited[0].0));
        assert!(Arc::ptr_eq(&original[1].0, &edited[1].0));
        // A mutable search visits the image row without copying its payload.
        edited
            .iter_mut()
            .find(|item| item["id"] == "prose")
            .unwrap()["text"] = json!("edited");
        assert!(Arc::ptr_eq(&original[0].0, &edited[0].0));
        assert!(!Arc::ptr_eq(&original[1].0, &edited[1].0));
        assert_ne!(original.revision(), edited.revision());
        assert_eq!(original[1]["text"], "before");
        assert_eq!(edited[1]["text"], "edited");
        assert_eq!(serde_json::to_value(&original).unwrap(), serialized);
        let before_clear = edited.revision();
        edited.clear();
        assert!(edited.is_empty());
        assert_eq!(original.len(), 2);
        assert_ne!(edited.revision(), before_clear);
    }
    #[test]
    fn serialized_array_is_unchanged_and_restored_identity_is_fresh() {
        let json = json!([{"id":"tool", "result":{"content":[{"type":"image","data":"exact bytes"}]}, "structuredResult":[{"data":"exact bytes"}]}]);
        let items: TranscriptItems = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&items).unwrap(), json);
        let restored: TranscriptItems = serde_json::from_value(json.clone()).unwrap();
        assert_ne!(items.revision(), restored.revision());
        assert_eq!(items, restored);
        let values: Vec<Value> = items.into();
        assert_eq!(serde_json::to_value(values).unwrap(), json);
    }
}
