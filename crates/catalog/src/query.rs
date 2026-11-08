//! Finding photos: the Library Filter bar and smart collection rules share one
//! criteria language, compiled to SQL over the photo query's tables
//! (`p` photos, `f` files, `fo` folders, `e` exif).

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

use crate::catalog::Flag;
use crate::scan::FileKind;

/// Text fields, as in LrC's Text filter and smart collection rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TextField {
    #[default]
    Any,
    FileName,
    Keywords,
    Title,
    Caption,
    Creator,
    Copyright,
    Camera,
    Lens,
    Folder,
}

impl TextField {
    pub const ALL: [TextField; 10] = [
        Self::Any,
        Self::FileName,
        Self::Keywords,
        Self::Title,
        Self::Caption,
        Self::Creator,
        Self::Copyright,
        Self::Camera,
        Self::Lens,
        Self::Folder,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Any Searchable Field",
            Self::FileName => "Filename",
            Self::Keywords => "Keywords",
            Self::Title => "Title",
            Self::Caption => "Caption",
            Self::Creator => "Creator",
            Self::Copyright => "Copyright",
            Self::Camera => "Camera",
            Self::Lens => "Lens",
            Self::Folder => "Folder",
        }
    }

    fn sql(self) -> String {
        const KEYWORDS: &str = "coalesce((SELECT group_concat(k.name, ' ') FROM photo_keywords pk
                                  JOIN keywords k ON k.id = pk.keyword_id WHERE pk.photo_id = p.id), '')";
        match self {
            Self::FileName => "f.name".into(),
            Self::Keywords => KEYWORDS.into(),
            Self::Title => "coalesce(p.title, '')".into(),
            Self::Caption => "coalesce(p.caption, '')".into(),
            Self::Creator => "coalesce(p.creator, '')".into(),
            Self::Copyright => "coalesce(p.copyright, '')".into(),
            Self::Camera => CAMERA_SQL.into(),
            Self::Lens => "coalesce(e.lens, '')".into(),
            Self::Folder => "fo.path".into(),
            Self::Any => Self::ALL[1..].iter().map(|f| f.sql()).collect::<Vec<_>>().join(" || ' ' || "),
        }
    }
}

pub(crate) const CAMERA_SQL: &str =
    "trim(coalesce(e.camera_make, '') || ' ' || coalesce(e.camera_model, ''))";
const YEAR: &str = "strftime('%Y', p.capture_time, 'unixepoch')";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TextOp {
    /// Every word appears somewhere (LrC's default).
    #[default]
    ContainsAll,
    /// At least one word appears.
    Contains,
    /// None of the words appear.
    DoesNotContain,
    StartsWith,
    EndsWith,
}

impl TextOp {
    pub const ALL: [TextOp; 5] =
        [Self::ContainsAll, Self::Contains, Self::DoesNotContain, Self::StartsWith, Self::EndsWith];

    pub fn label(self) -> &'static str {
        match self {
            Self::ContainsAll => "Contains All",
            Self::Contains => "Contains",
            Self::DoesNotContain => "Doesn't Contain",
            Self::StartsWith => "Starts With",
            Self::EndsWith => "Ends With",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum NumOp {
    #[default]
    AtLeast,
    AtMost,
    Equal,
}

impl NumOp {
    pub const ALL: [NumOp; 3] = [Self::AtLeast, Self::AtMost, Self::Equal];

    pub fn label(self) -> &'static str {
        match self {
            Self::AtLeast => "≥",
            Self::AtMost => "≤",
            Self::Equal => "=",
        }
    }
}

/// One condition on a photo. Trees of these describe filters and smart collections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Criterion {
    Text {
        field: TextField,
        op: TextOp,
        value: String,
    },
    Rating {
        op: NumOp,
        stars: u8,
    },
    Flag(Flag),
    /// `None`: no color label.
    Label(Option<String>),
    Kind(FileKind),
    /// Capture year; `None`: unknown date.
    Year(Option<i32>),
    /// Camera make + model as shown in the Metadata filter (`""` for unknown).
    Camera(String),
    Lens(String),
    /// Tagged with this keyword or any keyword inside it.
    Keyword(i64),
    /// File extension, any case (`"NEF"`).
    FileType(String),
    /// Captured within the last N days.
    CapturedWithinDays(u32),
    InQuickCollection,
    /// Matches if any child does (an empty list matches everything: "no filter").
    AnyOf(Vec<Criterion>),
    /// Matches if all children do (an empty list matches everything).
    AllOf(Vec<Criterion>),
    Not(Box<Criterion>),
}

/// Compare case-insensitively for all scripts (SQLite's own folding is ASCII-only).
pub(crate) const FOLD_FN: &str = "plfold";

fn bind(args: &mut Vec<Value>, v: impl Into<Value>) -> String {
    args.push(v.into());
    format!("?{}", args.len())
}

impl Criterion {
    /// A SQL condition (with parameters appended to `args`, numbered after any
    /// already there).
    pub fn to_sql(&self, args: &mut Vec<Value>) -> String {
        match self {
            Criterion::Text { field, op, value } => {
                let expr = format!("{FOLD_FN}({})", field.sql());
                let value = value.trim();
                if value.is_empty() {
                    return "1".into();
                }
                let words: Vec<&str> = value.split_whitespace().collect();
                let has = |args: &mut Vec<Value>, w: &str| {
                    let p = bind(args, w.to_owned());
                    format!("instr({expr}, {FOLD_FN}({p})) > 0")
                };
                match op {
                    TextOp::ContainsAll => {
                        words.iter().map(|w| has(args, w)).collect::<Vec<_>>().join(" AND ")
                    }
                    TextOp::Contains => {
                        format!("({})", words.iter().map(|w| has(args, w)).collect::<Vec<_>>().join(" OR "))
                    }
                    TextOp::DoesNotContain => words
                        .iter()
                        .map(|w| format!("NOT {}", has(args, w)))
                        .collect::<Vec<_>>()
                        .join(" AND "),
                    TextOp::StartsWith => {
                        let p = bind(args, value.to_owned());
                        format!("substr({expr}, 1, length({p})) = {FOLD_FN}({p})")
                    }
                    TextOp::EndsWith => {
                        let p = bind(args, value.to_owned());
                        format!("substr({expr}, -length({p})) = {FOLD_FN}({p})")
                    }
                }
            }
            Criterion::Rating { op, stars } => {
                let p = bind(args, i64::from(*stars));
                let cmp = match op {
                    NumOp::AtLeast => ">=",
                    NumOp::AtMost => "<=",
                    NumOp::Equal => "=",
                };
                format!("p.rating {cmp} {p}")
            }
            Criterion::Flag(flag) => format!("p.flag = {}", bind(args, *flag as i64)),
            Criterion::Label(None) => "p.color_label IS NULL".into(),
            Criterion::Label(Some(l)) => format!("p.color_label = {}", bind(args, l.clone())),
            Criterion::Kind(k) => format!("f.kind = {}", bind(args, k.as_str().to_owned())),
            Criterion::Year(None) => "p.capture_time IS NULL".into(),
            Criterion::Year(Some(y)) => format!("{YEAR} = {}", bind(args, format!("{y:04}"))),
            Criterion::Camera(c) => format!("{CAMERA_SQL} = {}", bind(args, c.clone())),
            Criterion::Lens(l) => format!("coalesce(e.lens, '') = {}", bind(args, l.clone())),
            Criterion::Keyword(id) => format!(
                "EXISTS (SELECT 1 FROM photo_keywords pk WHERE pk.photo_id = p.id AND pk.keyword_id IN (
                     WITH RECURSIVE kt(id) AS (
                         SELECT {} UNION ALL SELECT k.id FROM keywords k JOIN kt ON k.parent_id = kt.id
                     ) SELECT id FROM kt))",
                bind(args, *id)
            ),
            Criterion::FileType(ext) => format!("upper(f.ext) = upper({})", bind(args, ext.clone())),
            Criterion::CapturedWithinDays(days) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                format!("p.capture_time >= {}", bind(args, now - i64::from(*days) * 86_400))
            }
            Criterion::InQuickCollection => "p.id IN (SELECT photo_id FROM quick_collection)".into(),
            Criterion::AnyOf(list) | Criterion::AllOf(list) if list.is_empty() => "1".into(),
            Criterion::AnyOf(list) => {
                format!("({})", list.iter().map(|c| c.to_sql(args)).collect::<Vec<_>>().join(" OR "))
            }
            Criterion::AllOf(list) => {
                format!("({})", list.iter().map(|c| c.to_sql(args)).collect::<Vec<_>>().join(" AND "))
            }
            Criterion::Not(c) => format!("NOT ({})", c.to_sql(args)),
        }
    }
}

/// A smart collection's rules (LrC: "Match [all|any] of the following rules").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartRules {
    pub match_all: bool,
    pub rules: Vec<Criterion>,
}

impl Default for SmartRules {
    fn default() -> Self {
        SmartRules { match_all: true, rules: vec![Criterion::Rating { op: NumOp::AtLeast, stars: 3 }] }
    }
}

impl SmartRules {
    pub fn criterion(&self) -> Criterion {
        if self.rules.is_empty() {
            // LrC: a smart collection with no rules matches nothing.
            return Criterion::Not(Box::new(Criterion::AllOf(vec![])));
        }
        if self.match_all {
            Criterion::AllOf(self.rules.clone())
        } else {
            Criterion::AnyOf(self.rules.clone())
        }
    }
}

/// Columns of the Metadata filter (LrC's column browser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MetaColumn {
    #[default]
    Date,
    Camera,
    Lens,
    Keyword,
    FileType,
    Label,
}

impl MetaColumn {
    pub const ALL: [MetaColumn; 6] =
        [Self::Date, Self::Camera, Self::Lens, Self::Keyword, Self::FileType, Self::Label];

    pub fn label(self) -> &'static str {
        match self {
            Self::Date => "Date",
            Self::Camera => "Camera",
            Self::Lens => "Lens",
            Self::Keyword => "Keyword",
            Self::FileType => "File Type",
            Self::Label => "Label",
        }
    }
}

/// One value in a Metadata filter column, with how many photos have it.
#[derive(Debug, Clone, PartialEq)]
pub struct Facet {
    pub label: String,
    pub count: i64,
    /// Selecting the value filters with this.
    pub criterion: Criterion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_serialize_for_storage() {
        let rules = SmartRules {
            match_all: false,
            rules: vec![
                Criterion::Text {
                    field: TextField::Keywords,
                    op: TextOp::ContainsAll,
                    value: "film fuji".into(),
                },
                Criterion::Flag(Flag::Picked),
                Criterion::Not(Box::new(Criterion::Label(None))),
            ],
        };
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(serde_json::from_str::<SmartRules>(&json).unwrap(), rules);
    }

    #[test]
    fn empty_groups_match_everything_and_params_are_numbered() {
        let mut args = vec![Value::Integer(7)];
        assert_eq!(Criterion::AnyOf(vec![]).to_sql(&mut args), "1");
        let sql = Criterion::AllOf(vec![
            Criterion::Flag(Flag::Picked),
            Criterion::Rating { op: NumOp::AtLeast, stars: 2 },
        ])
        .to_sql(&mut args);
        assert_eq!(sql, "(p.flag = ?2 AND p.rating >= ?3)");
        assert_eq!(args.len(), 3);
    }
}
