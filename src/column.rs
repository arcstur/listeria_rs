pub use crate::listeria_page::ListeriaPage;
pub use crate::listeria_list::ListeriaList;
pub use crate::render_wikitext::RendererWikitext;
pub use crate::render_tabbed_data::RendererTabbedData;
pub use crate::result_row::ResultRow;

use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone)]
pub enum ColumnType {
    Number,
    Label,
    LabelLang(String),
    Description,
    Item,
    Property(String),
    PropertyQualifier((String, String)),
    PropertyQualifierValue((String, String, String)),
    Field(String),
    Unknown,
}

impl ColumnType {
    pub fn new(s: &str) -> Self {
        lazy_static! {
            static ref RE_LABEL_LANG: Regex = RegexBuilder::new(r#"^label/(.+)$"#)
                .case_insensitive(true)
                .build()
                .unwrap();
            static ref RE_PROPERTY: Regex = Regex::new(r#"^([Pp]\d+)$"#).unwrap();
            static ref RE_PROP_QUAL: Regex =
                Regex::new(r#"^\s*([Pp]\d+)\s*/\s*([Pp]\d+)\s*$"#).unwrap();
            static ref RE_PROP_QUAL_VAL: Regex =
                Regex::new(r#"^\s*([Pp]\d+)\s*/\s*([Qq]\d+)\s*/\s*([Pp]\d+)\s*$"#).unwrap();
            static ref RE_FIELD: Regex = Regex::new(r#"^\?(.+)$"#).unwrap();
        }
        match s.to_lowercase().as_str() {
            "number" => return ColumnType::Number,
            "label" => return ColumnType::Label,
            "description" => return ColumnType::Description,
            "item" => return ColumnType::Item,
            _ => {}
        }
        if let Some(caps) = RE_LABEL_LANG.captures(&s) {
            return ColumnType::LabelLang(
                caps.get(1).unwrap().as_str().to_lowercase(),
            )
        }
        if let Some(caps) = RE_PROPERTY.captures(&s) {
            return ColumnType::Property(
                caps.get(1).unwrap().as_str().to_uppercase(),
            )
        }
        if let Some(caps) = RE_PROP_QUAL.captures(&s) {
            return ColumnType::PropertyQualifier((
                caps.get(1).unwrap().as_str().to_uppercase(),
                caps.get(2).unwrap().as_str().to_uppercase(),
            ))
        }
        if let Some(caps) = RE_PROP_QUAL_VAL.captures(&s) {
            return ColumnType::PropertyQualifierValue((
                caps.get(1).unwrap().as_str().to_uppercase(),
                caps.get(2).unwrap().as_str().to_uppercase(),
                caps.get(3).unwrap().as_str().to_uppercase(),
            ))
        }
        if let Some(caps) = RE_FIELD.captures(&s) { return ColumnType::Field(caps.get(1).unwrap().as_str().to_string()) }
        ColumnType::Unknown
    }

    pub fn as_key(&self) -> String {
        match self {
            Self::Number => "number".to_string(),
            Self::Label => "label".to_string(),
            //Self::LabelLang(s) => {}
            Self::Description => "desc".to_string(),
            Self::Item => "item".to_string(),
            Self::Property(p) => p.to_lowercase(),
            Self::PropertyQualifier((p, q)) => p.to_lowercase() + "_" + &q.to_lowercase(),
            Self::PropertyQualifierValue((p, q, v)) => {
                p.to_lowercase() + "_" + &q.to_lowercase() + "_" + &v.to_lowercase()
            }
            Self::Field(f) => f.to_lowercase(),
            //Self::Unknown => ""
            _ => "unknown".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub obj: ColumnType,
    pub label: String,
}

impl Column {
    pub fn new(s: &str) -> Self {
        lazy_static! {
            static ref RE_COLUMN_LABEL: Regex = Regex::new(r#"^\s*(.+?)\s*:\s*(.+?)\s*$"#).unwrap();
        }
        match RE_COLUMN_LABEL.captures(&s) {
            Some(caps) => Self {
                obj: ColumnType::new(&caps.get(1).unwrap().as_str().to_string()),
                label: caps.get(2).unwrap().as_str().to_string(),
            },
            None => Self {
                obj: ColumnType::new(&s.trim().to_string()),
                label: s.trim().to_string(),
            },
        }
    }

    pub fn generate_label(&mut self, list: &ListeriaList) {
        self.label = match &self.obj {
            ColumnType::Property(prop) => list.get_label_with_fallback(prop),
            ColumnType::PropertyQualifier((prop, qual)) => {
                list.get_label_with_fallback(&prop)
                    + "/"
                    + &list.get_label_with_fallback(&qual)
            }
            ColumnType::PropertyQualifierValue((prop1, _qual, _prop2)) => {
                list.get_label_with_fallback(&prop1)
                    + "/"
                    + &list
                        .get_label_with_fallback(&prop1) // TODO FIXME
                    + "/"
                    + &list
                        .get_label_with_fallback(&prop1) // TODO FIXME
            }
            _ => self.label.to_owned(), // Fallback
        } ;
    }
}
