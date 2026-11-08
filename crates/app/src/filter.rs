//! The Library Filter bar's state (Text, Attribute, Metadata), and the catalog
//! query it amounts to.

use catalog::{Criterion, FileKind, Flag, MetaColumn, NumOp, TextField, TextOp};
use serde::{Deserialize, Serialize};

pub const COLUMNS: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterBar {
    /// Library ▸ Enable Filters (Ctrl+L). Off: everything shows, settings are kept.
    pub enabled: bool,
    pub text_on: bool,
    pub attribute_on: bool,
    pub metadata_on: bool,
    pub text_field: TextField,
    pub text_op: TextOp,
    pub text: String,
    /// Flags shown (none selected: any flag).
    pub flags: Vec<Flag>,
    pub rating_op: NumOp,
    /// 0: any rating.
    pub rating: u8,
    /// Labels shown; `None` is "no label".
    pub labels: Vec<Option<String>>,
    pub kinds: Vec<FileKind>,
    pub columns: [MetaColumn; COLUMNS],
    /// Values picked in each Metadata column (not remembered between runs).
    #[serde(skip)]
    pub picks: [Vec<Criterion>; COLUMNS],
}

impl Default for FilterBar {
    fn default() -> Self {
        FilterBar {
            enabled: true,
            text_on: false,
            attribute_on: false,
            metadata_on: false,
            text_field: TextField::Any,
            text_op: TextOp::ContainsAll,
            text: String::new(),
            flags: Vec::new(),
            rating_op: NumOp::AtLeast,
            rating: 0,
            labels: Vec::new(),
            kinds: Vec::new(),
            columns: [MetaColumn::Date, MetaColumn::Camera, MetaColumn::Lens, MetaColumn::Keyword],
            picks: Default::default(),
        }
    }
}

fn any_of<T>(items: &[T], f: impl Fn(&T) -> Criterion) -> Option<Criterion> {
    (!items.is_empty()).then(|| Criterion::AnyOf(items.iter().map(f).collect()))
}

impl FilterBar {
    /// The whole filter.
    pub fn criterion(&self) -> Criterion {
        self.criterion_before(COLUMNS)
    }

    /// The filter without Metadata columns from `column` on: LrC's columns narrow
    /// left to right, so a column lists the values left after the ones before it.
    pub fn criterion_before(&self, column: usize) -> Criterion {
        if !self.enabled {
            return Criterion::AllOf(vec![]);
        }
        let mut all = Vec::new();
        if self.text_on && !self.text.trim().is_empty() {
            all.push(Criterion::Text { field: self.text_field, op: self.text_op, value: self.text.clone() });
        }
        if self.attribute_on {
            all.extend(any_of(&self.flags, |f| Criterion::Flag(*f)));
            if self.rating > 0 {
                all.push(Criterion::Rating { op: self.rating_op, stars: self.rating });
            }
            all.extend(any_of(&self.labels, |l| Criterion::Label(l.clone())));
            all.extend(any_of(&self.kinds, |k| Criterion::Kind(*k)));
        }
        if self.metadata_on {
            for picks in self.picks.iter().take(column) {
                all.extend(any_of(picks, Criterion::clone));
            }
        }
        Criterion::AllOf(all)
    }

    /// Is anything actually being filtered out?
    pub fn is_filtering(&self) -> bool {
        self.criterion() != Criterion::AllOf(vec![])
    }

    /// LrC's "None": turn every filter off (their settings are kept).
    pub fn none(&mut self) {
        self.text_on = false;
        self.attribute_on = false;
        self.metadata_on = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_combined_query() {
        let mut f = FilterBar::default();
        assert_eq!(f.criterion(), Criterion::AllOf(vec![]));
        f.text = "hegra".into();
        f.flags = vec![Flag::Picked];
        f.rating = 3;
        assert!(!f.is_filtering(), "settings without their filter turned on do nothing");

        f.text_on = true;
        f.attribute_on = true;
        f.metadata_on = true;
        f.picks[0] = vec![Criterion::Year(Some(2026))];
        f.picks[1] = vec![Criterion::Camera("Nikon D750".into()), Criterion::Camera("".into())];
        let Criterion::AllOf(parts) = f.criterion() else { panic!() };
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[1], Criterion::AnyOf(vec![Criterion::Flag(Flag::Picked)]));
        assert_eq!(parts[2], Criterion::Rating { op: NumOp::AtLeast, stars: 3 });
        // Column 1's values are listed from what column 0 leaves.
        let Criterion::AllOf(before) = f.criterion_before(1) else { panic!() };
        assert_eq!(before.len(), 4);

        f.enabled = false;
        assert!(!f.is_filtering());
        f.enabled = true;
        f.none();
        assert!(!f.is_filtering());
    }
}
