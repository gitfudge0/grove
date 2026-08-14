use crate::IndexPath;

/// Passed as a slice to [`SearchableListDelegate::on_will_change`]; the delegate may apply all, some, or none by mutating `selection` directly.
pub enum SearchableListChange {
    Select { index: IndexPath },
    Deselect { index: IndexPath },
}
