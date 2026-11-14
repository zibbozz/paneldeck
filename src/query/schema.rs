//! Schema: welche Felder, Prädikate, Sortier-Schlüsselwörter und Funktionen es gibt. Wird vom Nutzer konfiguriert;
//! der Parser kennt es nicht, nur Validator und Autovervollständigung.

use crate::state::Lang;

/// Datentyp eines Feldes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Number,
    /// Datum ohne Uhrzeit (`2026-11-14`)
    Date,
    /// Datum mit Uhrzeit (`2026-11-14T10:30`, ein reines Datum gilt als 00:00)
    DateTime,
    Bool,
    /// Auswahl aus einer festen Werteliste (Dropdown)
    Option(Vec<String>),
    /// freie Schlagworte (immer Mehrfachwert)
    Labels,
    /// Verweis auf einen anderen Eintrag über seinen Schlüssel (`PAN-12`)
    TicketRef,
    /// Verweis auf ein Terminal bzw. eine Sitzung (Name)
    Terminal,
}

/// Der Typ ohne Nutzdaten (für Fehlermeldungen und Funktionsregister).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Number,
    Date,
    DateTime,
    Bool,
    Option,
    Labels,
    TicketRef,
    Terminal,
}

impl FieldKind {
    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            FieldKind::Text => lang.t("Text", "text"),
            FieldKind::Number => lang.t("Zahl", "number"),
            FieldKind::Date => lang.t("Datum", "date"),
            FieldKind::DateTime => lang.t("Datum mit Uhrzeit", "date and time"),
            FieldKind::Bool => lang.t("Ja/Nein", "yes/no"),
            FieldKind::Option => lang.t("Auswahl", "dropdown"),
            FieldKind::Labels => lang.t("Labels", "labels"),
            FieldKind::TicketRef => lang.t("Schlüssel (z. B. PAN-12)", "key (e.g. PAN-12)"),
            FieldKind::Terminal => lang.t("Terminal", "terminal"),
        }
    }
}

/// Operatoren, wie der Validator sie unterscheidet (Vergleiche, Listen und Leer-Prüfung).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Eq,
    Ne,
    Contains,
    NotContains,
    Lt,
    Gt,
    Le,
    Ge,
    In,
    NotIn,
    IsEmpty,
    IsNotEmpty,
}

impl OpKind {
    /// Alle Operatoren in der Reihenfolge, in der die Autovervollständigung sie anbietet.
    pub const ALL: [OpKind; 12] = [
        OpKind::Eq,
        OpKind::Ne,
        OpKind::Contains,
        OpKind::NotContains,
        OpKind::Lt,
        OpKind::Gt,
        OpKind::Le,
        OpKind::Ge,
        OpKind::In,
        OpKind::NotIn,
        OpKind::IsEmpty,
        OpKind::IsNotEmpty,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            OpKind::Eq => "=",
            OpKind::Ne => "!=",
            OpKind::Contains => "~",
            OpKind::NotContains => "!~",
            OpKind::Lt => "<",
            OpKind::Gt => ">",
            OpKind::Le => "<=",
            OpKind::Ge => ">=",
            OpKind::In => "IN",
            OpKind::NotIn => "NOT IN",
            OpKind::IsEmpty => "IS EMPTY",
            OpKind::IsNotEmpty => "IS NOT EMPTY",
        }
    }
}

impl From<super::ast::CmpOp> for OpKind {
    fn from(op: super::ast::CmpOp) -> Self {
        use super::ast::CmpOp;
        match op {
            CmpOp::Eq => OpKind::Eq,
            CmpOp::Ne => OpKind::Ne,
            CmpOp::Contains => OpKind::Contains,
            CmpOp::NotContains => OpKind::NotContains,
            CmpOp::Lt => OpKind::Lt,
            CmpOp::Gt => OpKind::Gt,
            CmpOp::Le => OpKind::Le,
            CmpOp::Ge => OpKind::Ge,
        }
    }
}

impl FieldType {
    pub fn kind(&self) -> FieldKind {
        match self {
            FieldType::Text => FieldKind::Text,
            FieldType::Number => FieldKind::Number,
            FieldType::Date => FieldKind::Date,
            FieldType::DateTime => FieldKind::DateTime,
            FieldType::Bool => FieldKind::Bool,
            FieldType::Option(_) => FieldKind::Option,
            FieldType::Labels => FieldKind::Labels,
            FieldType::TicketRef => FieldKind::TicketRef,
            FieldType::Terminal => FieldKind::Terminal,
        }
    }

    /// Ist der Operator für diesen Typ sinnvoll?
    pub fn allows(&self, op: OpKind) -> bool {
        use OpKind::*;
        match self.kind() {
            FieldKind::Text | FieldKind::Labels => matches!(op, Eq | Ne | Contains | NotContains | In | NotIn | IsEmpty | IsNotEmpty),
            FieldKind::Number | FieldKind::Date | FieldKind::DateTime => {
                matches!(op, Eq | Ne | Lt | Gt | Le | Ge | In | NotIn | IsEmpty | IsNotEmpty)
            }
            FieldKind::Bool => matches!(op, Eq | Ne | IsEmpty | IsNotEmpty),
            FieldKind::Option | FieldKind::TicketRef | FieldKind::Terminal => matches!(op, Eq | Ne | In | NotIn | IsEmpty | IsNotEmpty),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub ty: FieldType,
    /// Mehrfachwert (z. B. mehrere Labels): `=` heißt dann „enthält den Wert“; nach solchen Feldern lässt sich nicht sortieren.
    pub multi: bool,
}

/// Eintrag im Funktionsregister. Funktionen werden als Wert verwendet (`due < currentDate()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncDef {
    pub name: String,
    pub min_args: usize,
    pub max_args: usize,
    /// Für welche Feldtypen der Rückgabewert passt
    pub accepts: Vec<FieldKind>,
}

impl FuncDef {
    /// Funktion ohne Argumente.
    pub fn new(name: &str, accepts: &[FieldKind]) -> Self {
        Self { name: name.to_string(), min_args: 0, max_args: 0, accepts: accepts.to_vec() }
    }

    pub fn with_args(mut self, min: usize, max: usize) -> Self {
        self.min_args = min;
        self.max_args = max;
        self
    }
}

/// Die eingebauten Funktionen. Neue Funktion = hier eine Zeile (oder `Schema::with_function`).
pub fn builtin_functions() -> Vec<FuncDef> {
    vec![
        FuncDef::new("activeSprint", &[FieldKind::Option, FieldKind::Text]),
        FuncDef::new("currentDate", &[FieldKind::Date, FieldKind::DateTime]),
        FuncDef::new("linkedTerminal", &[FieldKind::Terminal, FieldKind::Text]),
    ]
}

/// Groß-/Kleinschreibung ignorierender Vergleich (Unicode).
pub fn eq_ci(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schema {
    pub fields: Vec<FieldDef>,
    /// Namen der Prädikate für `IS name` (z. B. `favorite`, `blocked`)
    pub predicates: Vec<String>,
    /// Sortier-Schlüsselwörter für `ORDER BY name` (z. B. `favorites`)
    pub sort_keywords: Vec<String>,
    pub functions: Vec<FuncDef>,
}

impl Default for Schema {
    fn default() -> Self {
        Self::new()
    }
}

impl Schema {
    /// Leeres Schema, aber mit den eingebauten Funktionen.
    pub fn new() -> Self {
        Self { fields: Vec::new(), predicates: Vec::new(), sort_keywords: Vec::new(), functions: builtin_functions() }
    }

    pub fn with_field(mut self, name: &str, ty: FieldType) -> Self {
        let multi = ty == FieldType::Labels;
        self.fields.push(FieldDef { name: name.to_string(), ty, multi });
        self
    }

    pub fn with_multi_field(mut self, name: &str, ty: FieldType) -> Self {
        self.fields.push(FieldDef { name: name.to_string(), ty, multi: true });
        self
    }

    pub fn with_predicate(mut self, name: &str) -> Self {
        self.predicates.push(name.to_string());
        self
    }

    pub fn with_sort_keyword(mut self, name: &str) -> Self {
        self.sort_keywords.push(name.to_string());
        self
    }

    pub fn with_function(mut self, f: FuncDef) -> Self {
        self.functions.push(f);
        self
    }

    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| eq_ci(&f.name, name))
    }

    pub fn function(&self, name: &str) -> Option<&FuncDef> {
        self.functions.iter().find(|f| eq_ci(&f.name, name))
    }

    pub fn predicate(&self, name: &str) -> Option<&str> {
        self.predicates.iter().map(String::as_str).find(|p| eq_ci(p, name))
    }

    pub fn sort_keyword(&self, name: &str) -> Option<&str> {
        self.sort_keywords.iter().map(String::as_str).find(|p| eq_ci(p, name))
    }
}
