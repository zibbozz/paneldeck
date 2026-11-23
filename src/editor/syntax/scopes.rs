//! Grammar scopes (TextMate/Sublime names such as `keyword.control.rust`) → colour classes ([`Tok`]). The theme only
//! defines colours per class, so every grammar looks consistent and light/dark themes and the theme editor keep working.

use super::Tok;

/// Scope prefixes in priority order (more specific first). A scope matches a prefix if it equals it or continues with
/// a dot (`comment` matches `comment.line.rust`).
const RULES: &[(&str, Tok)] = &[
    ("comment", Tok::Comment),
    ("constant.character.escape", Tok::Escape),
    ("constant.other.placeholder", Tok::Escape),
    ("constant.numeric", Tok::Number),
    ("constant", Tok::Constant),
    ("string.regexp", Tok::String),
    ("string", Tok::String),
    ("keyword.operator", Tok::Operator),
    ("keyword.preprocessor", Tok::Preprocessor),
    ("keyword.directive", Tok::Preprocessor),
    ("keyword.control.directive", Tok::Preprocessor),
    ("keyword", Tok::Keyword),
    ("storage", Tok::Keyword),
    ("meta.preprocessor", Tok::Preprocessor),
    ("entity.name.function", Tok::Function),
    ("entity.name.tag", Tok::Tag),
    ("entity.name.section", Tok::Heading),
    ("entity.name.label", Tok::Function),
    ("entity.name.constant", Tok::Constant),
    ("entity.name", Tok::Type),
    ("entity.other.attribute-name", Tok::Attribute),
    ("entity.other.inherited-class", Tok::Type),
    ("support.type.property-name", Tok::Attribute),
    ("support.function", Tok::Function),
    ("support.type", Tok::Type),
    ("support.class", Tok::Type),
    ("support.constant", Tok::Constant),
    ("variable.function", Tok::Function),
    ("variable.language", Tok::Keyword),
    ("variable.parameter", Tok::Variable),
    ("markup.heading", Tok::Heading),
    ("markup.inserted", Tok::Inserted),
    ("markup.deleted", Tok::Deleted),
    ("markup.raw", Tok::String),
    ("markup.underline.link", Tok::Function),
    ("invalid", Tok::Invalid),
];

fn matches(scope: &str, prefix: &str) -> bool {
    scope.starts_with(prefix) && (scope.len() == prefix.len() || scope.as_bytes()[prefix.len()] == b'.')
}

/// Colour class of one scope name (`None` = no colour of its own; the next outer scope decides).
pub fn tok_for_scope(scope: &str, sigil_vars: bool) -> Option<Tok> {
    if let Some((_, t)) = RULES.iter().find(|(p, _)| matches(scope, p)) {
        return Some(*t);
    }
    (sigil_vars && matches(scope, "variable.other")).then_some(Tok::Variable)
}

/// Outer scopes that turn a plain string into a key (JSON/YAML keys, object literal keys).
pub fn is_key_scope(scope: &str) -> bool {
    matches(scope, "meta.mapping.key") || matches(scope, "meta.object-literal.key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_match_on_dot_boundaries() {
        assert_eq!(tok_for_scope("comment.line.double-slash.rust", false), Some(Tok::Comment));
        assert_eq!(tok_for_scope("keyword.operator.arithmetic.c", false), Some(Tok::Operator));
        assert_eq!(tok_for_scope("keyword.control.rust", false), Some(Tok::Keyword));
        assert_eq!(tok_for_scope("constant.numeric.integer.decimal", false), Some(Tok::Number));
        assert_eq!(tok_for_scope("constant.character.escape.c", false), Some(Tok::Escape));
        assert_eq!(tok_for_scope("entity.name.function.python", false), Some(Tok::Function));
        assert_eq!(tok_for_scope("entity.name.class.java", false), Some(Tok::Type));
        assert_eq!(tok_for_scope("keyword.preprocessor.autoit", false), Some(Tok::Preprocessor));
        assert_eq!(tok_for_scope("commentary", false), None, "nur an Punktgrenzen");
        assert_eq!(tok_for_scope("meta.block.rust", false), None);
        assert_eq!(tok_for_scope("variable.other.readwrite.shell", false), None);
        assert_eq!(tok_for_scope("variable.other.readwrite.shell", true), Some(Tok::Variable));
        assert!(is_key_scope("meta.mapping.key.json"));
    }
}
