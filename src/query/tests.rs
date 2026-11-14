//! Tests für Parser, Pretty-Printer, Validator und Autovervollständigung.

use super::*;
use crate::state::Lang;

// ---------------------------------------------------------------------------------------------
// Hilfen
// ---------------------------------------------------------------------------------------------

fn q(s: &str) -> Query {
    parse(s).unwrap_or_else(|e| panic!("{s:?} sollte parsen, Fehler: {e:?}"))
}

fn errs(s: &str) -> Vec<ParseError> {
    match parse(s) {
        Ok(q) => panic!("{s:?} sollte fehlschlagen, ergab {q:?}"),
        Err(e) => e,
    }
}

fn codes(s: &str) -> Vec<ErrorCode> {
    errs(s).into_iter().map(|e| e.code).collect()
}

fn expected(what: Expect, found: Found) -> ErrorCode {
    ErrorCode::Expected { what, found }
}

fn tok(s: &str) -> Found {
    Found::Token(s.to_string())
}

fn filter(s: &str) -> Expr {
    q(s).filter.expect("Filter erwartet")
}

fn cmp(field: &str, op: CmpOp, kind: ValueKind) -> Expr {
    Expr::Compare { field: Name::new(field), op, op_loc: Loc::default(), value: Value::new(kind) }
}

fn word(s: &str) -> ValueKind {
    ValueKind::Word(s.to_string())
}

fn test_schema() -> Schema {
    Schema::new()
        .with_field("priority", FieldType::Option(vec!["Highest".into(), "High".into(), "Medium".into(), "Low".into()]))
        .with_field("type", FieldType::Option(vec!["Epic".into(), "Story".into(), "Bug".into(), "Task".into()]))
        .with_field(
            "status",
            FieldType::Option(vec!["Open".into(), "In Progress".into(), "In Review".into(), "Development finished".into(), "Done".into()]),
        )
        .with_field("sprint", FieldType::Option(vec!["Sprint 1".into(), "Sprint 2".into()]))
        .with_field("labels", FieldType::Labels)
        .with_field("Release version", FieldType::Text)
        .with_field("due", FieldType::Date)
        .with_field("created", FieldType::DateTime)
        .with_field("estimate", FieldType::Number)
        .with_field("Größe", FieldType::Number)
        .with_field("archived", FieldType::Bool)
        .with_field("parent", FieldType::TicketRef)
        .with_field("shell", FieldType::Terminal)
        .with_multi_field("components", FieldType::Text)
        .with_predicate("favorite")
        .with_predicate("blocked")
        .with_sort_keyword("favorites")
}

fn issues(s: &str) -> Vec<ValidationIssue> {
    validate(&q(s), &test_schema())
}

fn issue_codes(s: &str) -> Vec<IssueCode> {
    issues(s).into_iter().map(|i| i.code).collect()
}

/// Vorschläge mit dem Cursor am Ende der Eingabe.
fn sug(input: &str) -> Vec<Suggestion> {
    suggest(input, input.len(), &test_schema())
}

/// Der Cursor steht dort, wo im Text ein `|` ist (das `|` wird entfernt).
fn sug_at(marked: &str) -> (String, Vec<Suggestion>) {
    let pos = marked.find('|').expect("| fehlt");
    let input = format!("{}{}", &marked[..pos], &marked[pos + 1..]);
    let r = suggest(&input, pos, &test_schema());
    (input, r)
}

fn labels(v: &[Suggestion]) -> Vec<&str> {
    v.iter().map(|s| s.label.as_str()).collect()
}

fn inserts(v: &[Suggestion]) -> Vec<&str> {
    v.iter().map(|s| s.insert_text.as_str()).collect()
}

// ---------------------------------------------------------------------------------------------
// Parser: Grammatik
// ---------------------------------------------------------------------------------------------

const EXAMPLE: &str = r#"type = Epic AND "Release version" = 2.0.0 AND status IN (Done, "Development finished") ORDER BY priority"#;

#[test]
fn parses_the_example_query() {
    let query = q(EXAMPLE);
    let Some(Expr::And(items)) = &query.filter else { panic!("And erwartet: {query:?}") };
    assert_eq!(items.len(), 3);
    assert_eq!(items[0], cmp("type", CmpOp::Eq, word("Epic")));
    assert_eq!(items[1], cmp("Release version", CmpOp::Eq, word("2.0.0")));
    let Expr::In { field, negated, values, .. } = &items[2] else { panic!() };
    assert_eq!(field.text, "status");
    assert!(!negated);
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].kind, word("Done"));
    assert_eq!(values[1].kind, ValueKind::Text("Development finished".into()));
    assert_eq!(query.order.len(), 1);
    assert_eq!(query.order[0].name.text, "priority");
    assert_eq!(query.order[0].dir, None);
}

#[test]
fn example_is_valid_for_the_test_schema() {
    assert!(issues(EXAMPLE).is_empty(), "{:?}", issues(EXAMPLE));
}

#[test]
fn empty_and_blank_input_is_an_empty_query() {
    assert_eq!(q(""), Query::default());
    assert_eq!(q("   \t\n "), Query::default());
    assert_eq!(Query::default().to_string(), "");
}

#[test]
fn order_by_without_filter() {
    let query = q("ORDER BY priority DESC");
    assert!(query.filter.is_none());
    assert_eq!(query.order.len(), 1);
    assert_eq!(query.order[0].dir, Some(SortDir::Desc));
}

#[test]
fn precedence_and_binds_tighter_than_or() {
    let e = filter("a = 1 OR b = 2 AND c = 3");
    let Expr::Or(items) = e else { panic!("Or erwartet") };
    assert_eq!(items.len(), 2);
    assert!(matches!(items[0], Expr::Compare { .. }));
    assert!(matches!(&items[1], Expr::And(v) if v.len() == 2));
}

#[test]
fn precedence_not_binds_tighter_than_and() {
    let e = filter("NOT a = 1 AND b = 2");
    let Expr::And(items) = e else { panic!("And erwartet") };
    assert!(matches!(&items[0], Expr::Not(inner) if matches!(**inner, Expr::Compare { .. })));
    assert!(matches!(items[1], Expr::Compare { .. }));
}

#[test]
fn parentheses_override_precedence() {
    let e = filter("(a = 1 OR b = 2) AND c = 3");
    let Expr::And(items) = e else { panic!("And erwartet") };
    assert!(matches!(&items[0], Expr::Or(v) if v.len() == 2));
}

#[test]
fn not_applies_to_a_parenthesized_group() {
    let e = filter("NOT (a = 1 OR b = 2)");
    let Expr::Not(inner) = e else { panic!("Not erwartet") };
    assert!(matches!(*inner, Expr::Or(_)));
}

#[test]
fn double_not_and_redundant_parentheses() {
    assert!(matches!(filter("NOT NOT a = 1"), Expr::Not(i) if matches!(*i, Expr::Not(_))));
    assert_eq!(filter("(((a = 1)))"), cmp("a", CmpOp::Eq, ValueKind::Number("1".into())));
}

#[test]
fn chains_are_flattened() {
    let Expr::And(v) = filter("a = 1 AND b = 2 AND c = 3 AND d = 4") else { panic!() };
    assert_eq!(v.len(), 4);
    let Expr::Or(v) = filter("a = 1 OR b = 2 OR c = 3") else { panic!() };
    assert_eq!(v.len(), 3);
}

#[test]
fn deeply_nested_but_valid() {
    let e = filter("((a = 1 OR (b = 2 AND NOT (c = 3 OR d = 4))) AND e = 5) OR f = 6");
    assert!(matches!(e, Expr::Or(_)));
}

#[test]
fn keywords_are_case_insensitive() {
    let a = q("status in (Done) and not x = 1 or y is not empty order by z desc, w asc");
    let b = q("status IN (Done) AND NOT x = 1 OR y IS NOT EMPTY ORDER BY z DESC, w ASC");
    assert_eq!(a, b);
    assert_eq!(q("a = 1 Order By x"), q("a = 1 ORDER BY x"));
}

#[test]
fn sort_by_is_a_synonym_for_order_by() {
    assert_eq!(q("a = 1 SORT BY favorites"), q("a = 1 ORDER BY favorites"));
    assert_eq!(q("sort by favorites, priority desc").order.len(), 2);
}

#[test]
fn order_by_multiple_terms_with_directions() {
    let query = q(r#"ORDER BY priority DESC, "Release version", due asc"#);
    let dirs: Vec<_> = query.order.iter().map(|t| (t.name.text.as_str(), t.dir)).collect();
    assert_eq!(dirs, vec![("priority", Some(SortDir::Desc)), ("Release version", None), ("due", Some(SortDir::Asc))]);
}

#[test]
fn all_comparison_operators() {
    for (src, op) in [
        ("=", CmpOp::Eq),
        ("!=", CmpOp::Ne),
        ("~", CmpOp::Contains),
        ("!~", CmpOp::NotContains),
        ("<", CmpOp::Lt),
        (">", CmpOp::Gt),
        ("<=", CmpOp::Le),
        (">=", CmpOp::Ge),
    ] {
        assert_eq!(filter(&format!("x {src} 1")), cmp("x", op, ValueKind::Number("1".into())), "{src}");
        assert_eq!(filter(&format!("x{src}1")), cmp("x", op, ValueKind::Number("1".into())), "ohne Leerzeichen: {src}");
    }
}

#[test]
fn in_and_not_in_lists() {
    let Expr::In { negated, values, .. } = filter("x IN (1)") else { panic!() };
    assert!(!negated);
    assert_eq!(values.len(), 1);
    let Expr::In { negated, values, .. } = filter("x NOT IN (1, 2, a, \"b c\", NULL)") else { panic!() };
    assert!(negated);
    assert_eq!(values.len(), 5);
    assert_eq!(values[4].kind, ValueKind::Null);
}

#[test]
fn several_in_lists_combined() {
    let Expr::And(v) = filter("a IN (1, 2) AND (b NOT IN (3) OR c IN (4, 5, 6))") else { panic!() };
    assert!(matches!(v[0], Expr::In { .. }));
    assert!(matches!(&v[1], Expr::Or(o) if o.len() == 2));
}

#[test]
fn is_empty_variants() {
    assert!(matches!(filter("x IS EMPTY"), Expr::IsEmpty { negated: false, .. }));
    assert!(matches!(filter("x IS NOT EMPTY"), Expr::IsEmpty { negated: true, .. }));
    assert_eq!(filter("x IS NULL"), filter("x IS EMPTY"));
    assert_eq!(filter("x IS NOT NULL"), filter("x is not empty"));
}

#[test]
fn predicates() {
    let Expr::Predicate { name, negated } = filter("IS favorite") else { panic!() };
    assert_eq!(name.text, "favorite");
    assert!(!negated);
    assert!(matches!(filter("IS NOT blocked"), Expr::Predicate { negated: true, .. }));
    assert!(matches!(filter("is \"has attachments\""), Expr::Predicate { ref name, .. } if name.text == "has attachments"));
    let Expr::And(v) = filter("IS favorite AND NOT IS blocked") else { panic!() };
    assert!(matches!(&v[1], Expr::Not(i) if matches!(**i, Expr::Predicate { .. })));
}

#[test]
fn value_kinds() {
    let kind = |s: &str| match filter(&format!("x = {s}")) {
        Expr::Compare { value, .. } => value.kind,
        other => panic!("{other:?}"),
    };
    assert_eq!(kind("42"), ValueKind::Number("42".into()));
    assert_eq!(kind("-7"), ValueKind::Number("-7".into()));
    assert_eq!(kind("3.14"), ValueKind::Number("3.14".into()));
    assert_eq!(kind("2026-11-14"), ValueKind::Date("2026-11-14".into()));
    assert_eq!(kind("2026-11-14T10:30"), ValueKind::DateTime("2026-11-14T10:30".into()));
    assert_eq!(kind("2026-11-14T10:30:15"), ValueKind::DateTime("2026-11-14T10:30:15".into()));
    assert_eq!(kind("2.0.0"), word("2.0.0"));
    assert_eq!(kind("PAN-12"), word("PAN-12"));
    assert_eq!(kind("Done"), word("Done"));
    assert_eq!(kind("\"Done\""), ValueKind::Text("Done".into()));
    assert_eq!(kind("'In Review'"), ValueKind::Text("In Review".into()));
    assert_eq!(kind("NULL"), ValueKind::Null);
    assert_eq!(kind("empty"), ValueKind::Null);
    assert_eq!(kind("\"42\""), ValueKind::Text("42".into()));
}

#[test]
fn functions_as_values() {
    let Expr::Compare { value, .. } = filter("due < currentDate()") else { panic!() };
    let ValueKind::Func { name, args } = value.kind else { panic!() };
    assert_eq!(name.text, "currentDate");
    assert!(args.is_empty());
    let Expr::Compare { value, .. } = filter("sprint = activeSprint()") else { panic!() };
    assert!(matches!(value.kind, ValueKind::Func { .. }));
    let Expr::In { values, .. } = filter("shell IN (linkedTerminal(), main)") else { panic!() };
    assert!(matches!(values[0].kind, ValueKind::Func { .. }));
    // Funktionen mit Argumenten werden geparst (die Anzahl prüft der Validator)
    let Expr::Compare { value, .. } = filter("x = f(1, \"a\", g())") else { panic!() };
    let ValueKind::Func { args, .. } = value.kind else { panic!() };
    assert_eq!(args.len(), 3);
}

#[test]
fn quoted_names_double_and_single_quotes() {
    assert_eq!(filter(r#""Release version" = 1"#), filter("'Release version' = 1"));
    let Expr::Compare { field, .. } = filter(r#""Release version" = 1"#) else { panic!() };
    assert_eq!(field.text, "Release version");
}

#[test]
fn string_escapes() {
    let Expr::Compare { value, .. } = filter(r#"x = "a \"b\" \\ \n \t \r \' c""#) else { panic!() };
    assert_eq!(value.kind, ValueKind::Text("a \"b\" \\ \n \t \r ' c".into()));
    let Expr::Compare { value, .. } = filter(r#"x = 'it\'s "fine"'"#) else { panic!() };
    assert_eq!(value.kind, ValueKind::Text("it's \"fine\"".into()));
}

#[test]
fn free_text_search() {
    assert_eq!(filter(r#""login bug""#), Expr::Text("login bug".into(), Loc::default()));
    let Expr::And(v) = filter(r#""login bug" AND status = Done"#) else { panic!() };
    assert!(matches!(&v[0], Expr::Text(s, _) if s == "login bug"));
    let query = q(r#""abc" ORDER BY priority"#);
    assert!(matches!(query.filter, Some(Expr::Text(..))));
}

#[test]
fn quoted_keywords_are_plain_text() {
    let Expr::Compare { value, .. } = filter(r#"status = "and""#) else { panic!() };
    assert_eq!(value.kind, ValueKind::Text("and".into()));
    let Expr::Compare { field, .. } = filter(r#""order" = 1"#) else { panic!() };
    assert_eq!(field.text, "order");
}

#[test]
fn unicode_names_and_values() {
    let query = q("Größe >= 5 AND \"Fälligkeit (Ende)\" = 2026-11-14 AND Beschreibung ~ \"Müll äöü ß\" ORDER BY Größe");
    let Some(Expr::And(items)) = query.filter else { panic!() };
    assert_eq!(items.len(), 3);
    assert!(matches!(&items[0], Expr::Compare { field, .. } if field.text == "Größe"));
    assert!(matches!(&items[1], Expr::Compare { field, .. } if field.text == "Fälligkeit (Ende)"));
    assert_eq!(query.order[0].name.text, "Größe");
}

#[test]
fn spans_are_byte_offsets() {
    let src = "Größe > 5 AND x = Äpfel";
    let Some(Expr::And(items)) = q(src).filter else { panic!() };
    let Expr::Compare { field, value, op_loc, .. } = &items[0] else { panic!() };
    assert_eq!(&src[field.span().range()], "Größe");
    assert_eq!(field.span(), Span::new(0, "Größe".len()));
    assert_eq!(&src[op_loc.0.range()], ">");
    assert_eq!(&src[value.span().range()], "5");
    let Expr::Compare { field, value, .. } = &items[1] else { panic!() };
    assert_eq!(&src[field.span().range()], "x");
    assert_eq!(&src[value.span().range()], "Äpfel");
    assert_eq!(filter(src).span(), Span::new(0, src.len()));
}

#[test]
fn spans_of_quoted_strings_include_the_quotes() {
    let src = r#""Release version" = "a b""#;
    let Expr::Compare { field, value, .. } = filter(src) else { panic!() };
    assert_eq!(&src[field.span().range()], r#""Release version""#);
    assert_eq!(&src[value.span().range()], r#""a b""#);
}

// ---------------------------------------------------------------------------------------------
// Parser: Fehler
// ---------------------------------------------------------------------------------------------

#[test]
fn error_unknown_character() {
    let e = errs("status = Done @");
    assert_eq!(e[0].code, ErrorCode::UnexpectedChar('@'));
    assert_eq!(e[0].span, Span::new(14, 1));
    assert_eq!(errs("a = 1 # b")[0].span, Span::new(6, 1));
    assert_eq!(errs("a ! b")[0].code, ErrorCode::UnexpectedChar('!'));
}

#[test]
fn error_unknown_character_with_umlaut_before() {
    let e = errs("Größe = § AND x = 1");
    assert_eq!(e[0].code, ErrorCode::UnexpectedChar('§'));
    assert_eq!(e[0].span, Span::new("Größe = ".len(), '§'.len_utf8()));
}

#[test]
fn error_unterminated_string() {
    let e = errs("a = \"offen");
    assert!(e.iter().any(|e| e.code == ErrorCode::UnterminatedString && e.span == Span::new(4, 6)));
    assert_eq!(errs("a = 'x\\")[0].code, ErrorCode::UnterminatedString);
}

#[test]
fn error_invalid_escape() {
    let e = errs(r#"a = "x\qy""#);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].code, ErrorCode::InvalidEscape('q'));
    assert_eq!(e[0].span, Span::new(6, 2));
}

#[test]
fn error_missing_closing_paren() {
    let e = errs("(a = 1 AND b = 2");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].code, ErrorCode::UnclosedParen);
    assert_eq!(e[0].span, Span::new(0, 1));
    let e = errs("a = 1 AND (b = 2 OR (c = 3)");
    assert_eq!(e, vec![ParseError { span: Span::new(10, 1), code: ErrorCode::UnclosedParen }]);
}

#[test]
fn error_unmatched_closing_paren() {
    let e = errs("a = 1)");
    assert_eq!(e, vec![ParseError { span: Span::new(5, 1), code: ErrorCode::UnmatchedParen }]);
    assert_eq!(errs(") a = 1")[0].code, ErrorCode::UnmatchedParen);
}

#[test]
fn error_missing_value_at_end() {
    let e = errs("status =");
    assert_eq!(e, vec![ParseError { span: Span::new(8, 0), code: expected(Expect::Value, Found::End) }]);
}

#[test]
fn error_missing_value_before_keyword_is_reported_once() {
    let e = errs("status = AND x = 1");
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].code, expected(Expect::Value, tok("AND")));
    assert_eq!(e[0].span, Span::new(9, 3));
}

#[test]
fn error_word_instead_of_operator() {
    let e = errs("type Epic");
    assert_eq!(e, vec![ParseError { span: Span::new(5, 4), code: expected(Expect::Operator, tok("Epic")) }]);
    assert_eq!(errs("type")[0].code, expected(Expect::Operator, Found::End));
    assert_eq!(errs("type (")[0].code, expected(Expect::Operator, tok("(")));
}

#[test]
fn error_unexpected_token_after_condition() {
    let e = errs("a = 1 b = 2");
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].code, expected(Expect::LogicOrEnd, tok("b")));
    assert_eq!(e[0].span, Span::new(6, 1));
    // ein Operator dort, wo ein AND stehen müsste
    assert_eq!(errs("a = 1 = 2")[0].code, expected(Expect::LogicOrEnd, tok("=")));
}

#[test]
fn error_duplicate_order_by() {
    let e = errs("a = 1 ORDER BY x SORT BY y");
    assert_eq!(e, vec![ParseError { span: Span::new(17, 7), code: ErrorCode::DuplicateOrderBy }]);
}

#[test]
fn error_order_by_problems() {
    assert_eq!(errs("ORDER priority"), vec![ParseError { span: Span::new(6, 8), code: expected(Expect::By, tok("priority")) }]);
    assert_eq!(codes("ORDER BY"), vec![expected(Expect::OrderField, Found::End)]);
    assert_eq!(codes("ORDER BY a,"), vec![expected(Expect::OrderField, Found::End)]);
    assert_eq!(codes("ORDER BY a b"), vec![expected(Expect::OrderTail, tok("b"))]);
    assert_eq!(codes("ORDER BY AND"), vec![expected(Expect::OrderField, tok("AND"))]);
}

#[test]
fn error_in_list_problems() {
    assert_eq!(codes("x IN ()"), vec![ErrorCode::EmptyList]);
    assert_eq!(errs("x IN ()")[0].span, Span::new(5, 2));
    assert_eq!(errs("x IN (a, b")[0], ParseError { span: Span::new(5, 1), code: ErrorCode::UnclosedParen });
    assert_eq!(codes("x IN a"), vec![expected(Expect::OpenParen, tok("a"))]);
    assert_eq!(codes("x IN (a b)"), vec![expected(Expect::ListSeparator, tok("b"))]);
    assert_eq!(codes("x IN (a,)"), vec![expected(Expect::Value, tok(")"))]);
    assert_eq!(codes("x NOT a"), vec![expected(Expect::In, tok("a"))]);
    assert_eq!(codes("x IN"), vec![expected(Expect::OpenParen, Found::End)]);
}

#[test]
fn error_is_problems() {
    assert_eq!(codes("x IS"), vec![expected(Expect::EmptyKeyword, Found::End)]);
    assert_eq!(codes("x IS foo"), vec![expected(Expect::EmptyKeyword, tok("foo"))]);
    assert_eq!(codes("IS"), vec![expected(Expect::PredicateName, Found::End)]);
    assert_eq!(codes("IS EMPTY"), vec![expected(Expect::PredicateName, tok("EMPTY"))]);
    assert_eq!(codes("IS NOT"), vec![expected(Expect::PredicateName, Found::End)]);
}

#[test]
fn error_leading_logic_operator() {
    assert_eq!(codes("AND x = 1"), vec![expected(Expect::Condition, tok("AND"))]);
    assert_eq!(codes("x = 1 AND"), vec![expected(Expect::Condition, Found::End)]);
    assert_eq!(codes("x = 1 OR OR y = 2"), vec![expected(Expect::Condition, tok("OR"))]);
}

#[test]
fn several_errors_are_collected_and_sorted() {
    let e = errs("status = AND type Epic AND (a = 1 ORDER BY");
    let starts: Vec<usize> = e.iter().map(|e| e.span.start).collect();
    assert!(e.len() >= 3, "{e:?}");
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "nicht sortiert: {e:?}");
    assert!(e.iter().any(|e| e.code == expected(Expect::Value, tok("AND"))));
    assert!(e.iter().any(|e| e.code == expected(Expect::Operator, tok("Epic"))));
    assert!(e.iter().any(|e| e.code == ErrorCode::UnclosedParen));
}

#[test]
fn recovery_continues_after_an_error_inside_parentheses() {
    let e = errs("(a = ) AND b ~ AND c = 1");
    assert_eq!(e.len(), 2, "{e:?}");
    assert_eq!(e[0].code, expected(Expect::Value, tok(")")));
    assert_eq!(e[1].code, expected(Expect::Value, tok("AND")));
}

#[test]
fn reserved_word_as_value_gets_a_hint() {
    let e = &errs("title ~ not")[0];
    assert_eq!(e.code, expected(Expect::Value, tok("not")));
    assert!(e.message(Lang::En).contains("reserved"));
    assert!(e.message(Lang::De).contains("reserviert"));
}

#[test]
fn pathological_nesting_does_not_overflow_the_stack() {
    let deep = "(".repeat(50_000);
    assert!(errs(&deep).iter().any(|e| e.code == ErrorCode::TooDeep));
    let nots = "NOT ".repeat(50_000) + "a = 1";
    assert!(errs(&nots).iter().any(|e| e.code == ErrorCode::TooDeep));
    let ok = format!("{}a = 1{}", "(".repeat(100), ")".repeat(100));
    assert!(parse(&ok).is_ok());
}

#[test]
fn error_messages_exist_in_both_languages() {
    let samples = [
        "a = @", "a = \"x", r#"a = "\q""#, "(a = 1", "a = 1)", "ORDER BY a ORDER BY b", "a IN ()", "((((", "x", "x =", "x = AND", "x IN a", "x NOT a",
        "x IS foo", "IS", "ORDER", "ORDER BY", "a = 1 b", "ORDER BY a b", "x IN (a b)", "AND", "x = 1 AND",
    ];
    for s in samples {
        for e in errs(s) {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty(), "{s}");
            assert_ne!(de, en, "gleich in beiden Sprachen: {s} -> {de}");
        }
    }
    assert_eq!(errs("a = @")[0].message(Lang::En), "Unknown character “@”");
    assert_eq!(errs("a = @")[0].message(Lang::De), "Unbekanntes Zeichen „@“");
    assert_eq!(errs("status =")[0].message(Lang::En), "Missing value");
}

// ---------------------------------------------------------------------------------------------
// Pretty-Printer und Rundreise
// ---------------------------------------------------------------------------------------------

const ROUND_TRIP: &[&str] = &[
    EXAMPLE,
    "",
    "a = 1",
    "a = 1 OR b = 2 AND c = 3",
    "(a = 1 OR b = 2) AND c = 3",
    "NOT a = 1 AND b = 2",
    "NOT (a = 1 AND b = 2)",
    "NOT NOT a = 1",
    "(a = 1 AND b = 2) AND c = 3",
    "a = 1 AND (b = 2 AND c = 3)",
    "(a = 1 OR b = 2) OR c = 3",
    "a IN (1, 2, 3) AND b NOT IN (x, \"y z\")",
    "a IS EMPTY OR b IS NOT EMPTY",
    "IS favorite AND NOT IS NOT blocked",
    "x ~ \"a \\\"quoted\\\" \\\\ value\"",
    "x != 'single'",
    "due < currentDate() AND sprint = activeSprint() AND shell = linkedTerminal()",
    "x = f(1, \"a\", g())",
    "Größe >= 5 AND \"Fälligkeit (Ende)\" = 2026-11-14",
    "created > 2026-11-14T10:30:15",
    "\"login bug\" AND status = Done",
    "NOT \"abc\"",
    "status = NULL OR status = EMPTY",
    "x = \"and\" AND \"order\" = 1 AND \"IS\" IN (1)",
    "x = -5 AND y = 3.14 AND z = 2.0.0 AND p = PAN-12",
    "ORDER BY priority",
    "a = 1 ORDER BY priority DESC, \"Release version\" ASC, due",
    "SORT BY favorites",
    "a=1 and b<=2",
    "x = \"\"",
    "\"\"",
];

#[test]
fn round_trip_examples() {
    for src in ROUND_TRIP {
        let first = q(src);
        let printed = first.to_string();
        let second = parse(&printed).unwrap_or_else(|e| panic!("{src:?} -> {printed:?} lässt sich nicht lesen: {e:?}"));
        assert_eq!(first, second, "{src:?} -> {printed:?}");
        assert_eq!(printed, second.to_string(), "Ausgabe ist nicht stabil: {src:?}");
    }
}

#[test]
fn printer_output_for_known_inputs() {
    assert_eq!(q("a=1 and b in (x,y) order by  c desc").to_string(), "a = 1 AND b IN (x, y) ORDER BY c DESC");
    assert_eq!(q("SORT BY favorites").to_string(), "ORDER BY favorites");
    assert_eq!(q("'Release version' = '2.0.0'").to_string(), r#""Release version" = "2.0.0""#);
    assert_eq!(q("x = 2.0.0").to_string(), "x = 2.0.0");
    assert_eq!(q("x IS NULL").to_string(), "x IS EMPTY");
    assert_eq!(q("x = empty").to_string(), "x = NULL");
    assert_eq!(q(r#"x = "a\nb""#).to_string(), r#"x = "a\nb""#);
}

#[test]
fn printer_keeps_structure_with_parentheses() {
    assert_eq!(q("(a = 1 AND b = 2) AND c = 3").to_string(), "(a = 1 AND b = 2) AND c = 3");
    assert_eq!(q("(a = 1 OR b = 2) AND c = 3").to_string(), "(a = 1 OR b = 2) AND c = 3");
    assert_eq!(q("a = 1 OR (b = 2 AND c = 3)").to_string(), "a = 1 OR b = 2 AND c = 3");
    assert_eq!(q("NOT (a = 1)").to_string(), "NOT a = 1");
    assert_eq!(q("NOT (a = 1 OR b = 2)").to_string(), "NOT (a = 1 OR b = 2)");
}

#[test]
fn printer_quotes_names_only_when_needed() {
    assert_eq!(Name::new("status").to_string(), "status");
    assert_eq!(Name::new("Größe").to_string(), "Größe");
    assert_eq!(Name::new("Release version").to_string(), "\"Release version\"");
    assert_eq!(Name::new("and").to_string(), "\"and\"");
    assert_eq!(Name::new("").to_string(), "\"\"");
    assert_eq!(Name::new("a\"b").to_string(), "\"a\\\"b\"");
}

#[test]
fn printer_never_turns_a_hand_built_word_into_something_else() {
    // ein „Wort“, das gar keins ist, wird als Text ausgegeben und bleibt damit lesbar
    assert_eq!(Value::new(ValueKind::Word("12".into())).to_string(), "\"12\"");
    assert_eq!(Value::new(ValueKind::Word("a b".into())).to_string(), "\"a b\"");
    assert_eq!(Value::new(ValueKind::Word("and".into())).to_string(), "\"and\"");
}

/// Kleiner deterministischer Zufallsgenerator (xorshift), damit die Tests ohne Zusatzpaket reproduzierbar bleiben.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NAMES: &[&str] = &[
    "status", "priority", "Release version", "Größe", "and", "IS", "a-b", "x y z", "Fälligkeit", "quo\"te", "empty", "order", "2026", "a.b", "日本語", "in",
];
const WORDS: &[&str] = &["Done", "2.0.0", "PAN-12", "Größe", "x_y", "a:b", "ß", "v1.2"];
const TEXT_CHARS: &[char] = &['a', ' ', '"', '\\', '\n', 'ä', '\'', '(', ')', '=', ',', '\t', 'A', '1'];

fn gen_name(r: &mut Rng) -> Name {
    Name::new(*r.pick(NAMES))
}

fn gen_text(r: &mut Rng) -> String {
    (0..r.below(7)).map(|_| *r.pick(TEXT_CHARS)).collect()
}

fn gen_value(r: &mut Rng, depth: usize) -> Value {
    let kind = match r.below(if depth > 0 { 7 } else { 6 }) {
        0 => ValueKind::Number((*r.pick(&["5", "-3", "3.14", "0", "-0.5"])).into()),
        1 => ValueKind::Date("2026-11-14".into()),
        2 => ValueKind::DateTime((*r.pick(&["2026-11-14T10:30", "2026-11-14T10:30:15"])).into()),
        3 => ValueKind::Word((*r.pick(WORDS)).into()),
        4 => ValueKind::Text(gen_text(r)),
        5 => ValueKind::Null,
        _ => ValueKind::Func {
            name: Name::new(*r.pick(&["activeSprint", "currentDate", "f_1"])),
            args: (0..r.below(3)).map(|_| gen_value(r, depth - 1)).collect(),
        },
    };
    Value::new(kind)
}

fn gen_expr(r: &mut Rng, depth: usize) -> Expr {
    let kind = if depth == 0 { r.below(6) } else { r.below(9) };
    match kind {
        0 => Expr::Compare {
            field: gen_name(r),
            op: *r.pick(&[CmpOp::Eq, CmpOp::Ne, CmpOp::Contains, CmpOp::NotContains, CmpOp::Lt, CmpOp::Gt, CmpOp::Le, CmpOp::Ge]),
            op_loc: Loc::default(),
            value: gen_value(r, 1),
        },
        1 => Expr::In {
            field: gen_name(r),
            negated: r.below(2) == 0,
            op_loc: Loc::default(),
            values: (0..1 + r.below(3)).map(|_| gen_value(r, 1)).collect(),
        },
        2 => Expr::IsEmpty { field: gen_name(r), negated: r.below(2) == 0, op_loc: Loc::default() },
        3 => Expr::Predicate { name: gen_name(r), negated: r.below(2) == 0 },
        4 | 5 => Expr::Text(gen_text(r), Loc::default()),
        6 => Expr::Not(Box::new(gen_expr(r, depth - 1))),
        7 => Expr::And((0..2 + r.below(2)).map(|_| gen_expr(r, depth - 1)).collect()),
        _ => Expr::Or((0..2 + r.below(2)).map(|_| gen_expr(r, depth - 1)).collect()),
    }
}

fn gen_query(r: &mut Rng) -> Query {
    let filter = if r.below(6) == 0 { None } else { Some(gen_expr(r, 3)) };
    let count = r.below(4);
    let order = (0..count).map(|_| OrderTerm { name: gen_name(r), dir: *r.pick(&[None, Some(SortDir::Asc), Some(SortDir::Desc)]) }).collect();
    Query { filter, order }
}

#[test]
fn round_trip_property_on_generated_queries() {
    let mut r = Rng(0x2545_F491_4F6C_DD1D);
    for i in 0..3000 {
        let built = gen_query(&mut r);
        let printed = built.to_string();
        let parsed = parse(&printed).unwrap_or_else(|e| panic!("#{i}: {printed:?} lässt sich nicht lesen: {e:?}"));
        assert_eq!(built, parsed, "#{i}: {printed:?}");
        assert_eq!(printed, parsed.to_string(), "#{i}: Ausgabe nicht stabil");
    }
}

#[test]
fn parse_and_suggest_never_panic_on_random_input() {
    let alphabet: Vec<char> = "ab Größe\"'\\()=!~<>,.-:_@#|0129ANDORNTISEMPYBXäß日 \n".chars().collect();
    let schema = test_schema();
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..1500 {
        let count = r.below(25);
        let s: String = (0..count).map(|_| *r.pick(&alphabet)).collect();
        match parse(&s) {
            Ok(query) => {
                assert_eq!(parse(&query.to_string()).as_ref(), Ok(&query), "{s:?}");
                let _ = validate(&query, &schema);
            }
            Err(errors) => {
                assert!(!errors.is_empty());
                for e in &errors {
                    assert!(e.span.end() <= s.len(), "{s:?}: {e:?}");
                    assert!(s.is_char_boundary(e.span.start) && s.is_char_boundary(e.span.end()), "{s:?}: {e:?}");
                    let _ = e.message(Lang::De);
                }
            }
        }
        for c in 0..=s.len() + 1 {
            for sg in suggest(&s, c, &schema) {
                assert!(sg.replace_range.end <= s.len(), "{s:?}@{c}");
                assert!(s.is_char_boundary(sg.replace_range.start) && s.is_char_boundary(sg.replace_range.end), "{s:?}@{c}");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Validator
// ---------------------------------------------------------------------------------------------

#[test]
fn validator_unknown_field_with_suggestion() {
    assert_eq!(issue_codes("priorty = High"), vec![IssueCode::UnknownField { name: "priorty".into(), suggestion: Some("priority".into()) }]);
    assert_eq!(
        issue_codes("\"Relase version\" = 1"),
        vec![IssueCode::UnknownField { name: "Relase version".into(), suggestion: Some("Release version".into()) }]
    );
    // ein Präfix wird auch erkannt
    assert_eq!(issue_codes("stat = Done")[0], IssueCode::UnknownField { name: "stat".into(), suggestion: Some("status".into()) });
}

#[test]
fn validator_unknown_field_without_close_match() {
    assert_eq!(issue_codes("zzzzzz = 1"), vec![IssueCode::UnknownField { name: "zzzzzz".into(), suggestion: None }]);
}

#[test]
fn validator_field_names_are_case_insensitive_and_unicode() {
    assert!(issues("PRIORITY = high").is_empty());
    assert!(issues("größe > 3").is_empty());
    assert_eq!(issue_codes("Grösse > 3"), vec![IssueCode::UnknownField { name: "Grösse".into(), suggestion: Some("Größe".into()) }]);
}

#[test]
fn validator_unknown_field_skips_value_checks() {
    assert_eq!(issues("nope = High AND nope2 < 5").len(), 2);
}

#[test]
fn validator_operator_does_not_fit_the_type() {
    let op = |field: &str, op: OpKind, kind: FieldKind| IssueCode::OperatorNotAllowed { field: field.into(), op, kind };
    assert_eq!(issue_codes("estimate ~ 5"), vec![op("estimate", OpKind::Contains, FieldKind::Number)]);
    assert_eq!(issue_codes("priority < High"), vec![op("priority", OpKind::Lt, FieldKind::Option)]);
    assert_eq!(issue_codes("priority ~ Hi"), vec![op("priority", OpKind::Contains, FieldKind::Option)]);
    assert_eq!(issue_codes("archived > true"), vec![op("archived", OpKind::Gt, FieldKind::Bool)]);
    assert_eq!(issue_codes("archived IN (true)"), vec![op("archived", OpKind::In, FieldKind::Bool)]);
    assert_eq!(issue_codes("\"Release version\" >= 1"), vec![op("Release version", OpKind::Ge, FieldKind::Text)]);
    assert_eq!(issue_codes("due ~ 2026-01-01"), vec![op("due", OpKind::Contains, FieldKind::Date)]);
    assert_eq!(issue_codes("parent < PAN-1"), vec![op("parent", OpKind::Lt, FieldKind::TicketRef)]);
    assert_eq!(issue_codes("estimate NOT IN (1, 2)"), vec![]);
}

#[test]
fn validator_accepts_matching_operators() {
    for s in [
        "\"Release version\" ~ \"2.\"",
        "\"Release version\" !~ x",
        "estimate >= 3",
        "due < currentDate()",
        "created <= 2026-11-14T10:30",
        "labels ~ bug",
        "labels = bug",
        "status NOT IN (Open, Done)",
        "priority IS EMPTY",
        "archived IS NOT EMPTY",
        "archived = true",
        "parent = PAN-12",
        "parent IN (PAN-1, \"PAN-2\")",
        "shell = linkedTerminal()",
    ] {
        assert!(issues(s).is_empty(), "{s}: {:?}", issues(s));
    }
}

#[test]
fn validator_value_type_mismatch() {
    let mism = |field: &str, expected: FieldKind, found: &str| IssueCode::ValueTypeMismatch { field: field.into(), expected, found: found.into() };
    assert_eq!(issue_codes("estimate = abc"), vec![mism("estimate", FieldKind::Number, "abc")]);
    assert_eq!(issue_codes("estimate = 2026-11-14"), vec![mism("estimate", FieldKind::Number, "2026-11-14")]);
    assert_eq!(issue_codes("due = tomorrow"), vec![mism("due", FieldKind::Date, "tomorrow")]);
    assert_eq!(issue_codes("archived = maybe"), vec![mism("archived", FieldKind::Bool, "maybe")]);
    assert_eq!(issue_codes("parent = foo"), vec![mism("parent", FieldKind::TicketRef, "foo")]);
    assert_eq!(issue_codes("estimate IN (1, x, 3)"), vec![mism("estimate", FieldKind::Number, "x")]);
    // eine Zahl in Anführungszeichen ist erlaubt
    assert!(issues("estimate = \"5\"").is_empty());
    assert!(issues("archived = YES").is_empty());
}

#[test]
fn validator_dates() {
    assert_eq!(issue_codes("due = 2026-13-45"), vec![IssueCode::InvalidDate { found: "2026-13-45".into() }]);
    assert_eq!(issue_codes("due = 2026-02-30"), vec![IssueCode::InvalidDate { found: "2026-02-30".into() }]);
    assert_eq!(issue_codes("due = 2023-02-29"), vec![IssueCode::InvalidDate { found: "2023-02-29".into() }]);
    assert!(issues("due = 2024-02-29").is_empty());
    assert!(issues("due = 2000-02-29").is_empty());
    assert_eq!(issue_codes("due = 1900-02-29").len(), 1);
    assert!(issues("due = \"2026-11-14\"").is_empty());
    // ein Datumsfeld mit Uhrzeit ist ein Typfehler, kein „ungültiges Datum“
    assert_eq!(
        issue_codes("due = 2026-11-14T10:30"),
        vec![IssueCode::ValueTypeMismatch { field: "due".into(), expected: FieldKind::Date, found: "2026-11-14T10:30".into() }]
    );
}

#[test]
fn validator_date_times() {
    assert!(issues("created > 2026-11-14").is_empty());
    assert!(issues("created > 2026-11-14T23:59:59").is_empty());
    assert!(issues("created > \"2026-11-14 10:30\"").is_empty());
    assert_eq!(issue_codes("created > 2026-11-14T25:00"), vec![IssueCode::InvalidDate { found: "2026-11-14T25:00".into() }]);
    assert_eq!(issue_codes("created > 2026-11-14T10:61"), vec![IssueCode::InvalidDate { found: "2026-11-14T10:61".into() }]);
    assert_eq!(issue_codes("created > 2026-00-14").len(), 1);
    assert_eq!(issue_codes("created > nope").len(), 1);
}

#[test]
fn validator_functions() {
    assert!(issues("sprint = activeSprint()").is_empty());
    assert!(issues("due < CURRENTDATE()").is_empty());
    assert!(issues("created >= currentDate()").is_empty());
    assert_eq!(
        issue_codes("due < currentDat()"),
        vec![IssueCode::UnknownFunction { name: "currentDat".into(), suggestion: Some("currentDate".into()) }]
    );
    assert_eq!(issue_codes("due < foo()"), vec![IssueCode::UnknownFunction { name: "foo".into(), suggestion: None }]);
    assert_eq!(
        issue_codes("due = activeSprint()"),
        vec![IssueCode::FunctionNotAllowed { name: "activeSprint".into(), field: "due".into(), kind: FieldKind::Date }]
    );
    assert_eq!(issue_codes("due < currentDate(1)"), vec![IssueCode::FunctionArity { name: "currentDate".into(), min: 0, max: 0, got: 1 }]);
}

#[test]
fn validator_function_registry_is_extensible() {
    let schema = test_schema().with_function(FuncDef::new("startOfWeek", &[FieldKind::Date]).with_args(0, 1));
    for s in ["due = startOfWeek()", "due = startOfWeek(1)"] {
        assert!(validate(&q(s), &schema).is_empty(), "{s}");
    }
    assert_eq!(validate(&q("due = startOfWeek(1, 2)"), &schema).len(), 1);
    // ohne Eintrag unbekannt
    assert_eq!(issue_codes("due = startOfWeek()").len(), 1);
}

#[test]
fn validator_predicates() {
    assert!(issues("IS favorite AND IS NOT blocked AND NOT IS FAVORITE").is_empty());
    assert_eq!(issue_codes("IS favourit"), vec![IssueCode::UnknownPredicate { name: "favourit".into(), suggestion: Some("favorite".into()) }]);
    assert_eq!(issue_codes("IS zzzzzzzz"), vec![IssueCode::UnknownPredicate { name: "zzzzzzzz".into(), suggestion: None }]);
}

#[test]
fn validator_option_values() {
    assert!(issues("status = done").is_empty());
    assert!(issues("status = \"in progress\"").is_empty());
    assert_eq!(
        issue_codes("status = Don"),
        vec![IssueCode::OptionNotAllowed { field: "status".into(), value: "Don".into(), suggestion: Some("Done".into()) }]
    );
    assert_eq!(
        issue_codes("status IN (Done, \"Development finishd\", Zzzzzz)"),
        vec![
            IssueCode::OptionNotAllowed { field: "status".into(), value: "Development finishd".into(), suggestion: Some("Development finished".into()) },
            IssueCode::OptionNotAllowed { field: "status".into(), value: "Zzzzzz".into(), suggestion: None },
        ]
    );
}

#[test]
fn validator_null_values() {
    assert!(issues("status = NULL").is_empty());
    assert!(issues("status != EMPTY").is_empty());
    assert!(issues("status IN (NULL, Done)").is_empty());
    assert_eq!(issue_codes("estimate > NULL"), vec![IssueCode::NullOperator { op: OpKind::Gt }]);
}

#[test]
fn validator_order_by() {
    assert!(issues("ORDER BY priority DESC, favorites, \"Release version\", Größe").is_empty());
    assert_eq!(
        issue_codes("ORDER BY priorty"),
        vec![IssueCode::UnknownSortKey { name: "priorty".into(), suggestion: Some("priority".into()) }]
    );
    assert_eq!(
        issue_codes("ORDER BY favorits"),
        vec![IssueCode::UnknownSortKey { name: "favorits".into(), suggestion: Some("favorites".into()) }]
    );
    assert_eq!(issue_codes("ORDER BY components"), vec![IssueCode::NotSortable { name: "components".into() }]);
    assert_eq!(issue_codes("ORDER BY labels"), vec![IssueCode::NotSortable { name: "labels".into() }]);
}

#[test]
fn validator_issue_positions() {
    let src = "status = Don AND nope = 1 AND estimate ~ 5 ORDER BY zzzzzz";
    let list = issues(src);
    assert_eq!(list.len(), 4);
    let texts: Vec<&str> = list.iter().map(|i| &src[i.span.range()]).collect();
    assert_eq!(texts, vec!["Don", "nope", "~", "zzzzzz"]);
    assert!(list.windows(2).all(|w| w[0].span.start <= w[1].span.start));
}

#[test]
fn validator_issue_positions_with_umlauts() {
    let src = "Größe = abc AND Grösse > 2";
    let list = issues(src);
    assert_eq!(list.len(), 2);
    assert_eq!(&src[list[0].span.range()], "abc");
    assert_eq!(&src[list[1].span.range()], "Grösse");
}

#[test]
fn validator_messages_in_both_languages() {
    let list = issues(
        "priorty = x AND estimate ~ 5 AND estimate = abc AND due = 2026-13-45 AND due = foo() AND IS nope AND status = Don ORDER BY zzz, components",
    );
    assert!(list.len() >= 8, "{list:?}");
    for i in &list {
        let (de, en) = (i.message(Lang::De), i.message(Lang::En));
        assert!(!de.is_empty() && !en.is_empty());
        assert_ne!(de, en, "{i:?}");
    }
    assert_eq!(issues("priorty = High")[0].message(Lang::En), "Unknown field “priorty” – did you mean “priority”?");
    assert_eq!(issues("priorty = High")[0].message(Lang::De), "Unbekanntes Feld „priorty“ – meinten Sie „priority“?");
    assert_eq!(issues("priorty = High")[0].suggestion(), Some("priority"));
    assert_eq!(issues("estimate ~ 5")[0].message(Lang::En), "The operator “~” does not fit the field “estimate” (type: number)");
}

#[test]
fn levenshtein_and_closest() {
    assert_eq!(levenshtein("kitten", "sitting"), 3);
    assert_eq!(levenshtein("", "abc"), 3);
    assert_eq!(levenshtein("Größe", "GRÖSSE"), 2);
    assert_eq!(levenshtein("abc", "ABC"), 0);
    assert_eq!(closest("statsu", ["status", "type"]), Some("status".into()));
    assert_eq!(closest("xyz", ["status", "type"]), None);
    // gleich (ohne Beachtung der Schreibweise) ist kein Vorschlag
    assert_eq!(closest("type", ["TYPE"]), None);
}

// ---------------------------------------------------------------------------------------------
// Autovervollständigung
// ---------------------------------------------------------------------------------------------

#[test]
fn suggest_at_start_offers_fields_not_and_predicates() {
    let r = sug("");
    assert_eq!(
        labels(&r),
        vec![
            "archived", "components", "created", "due", "estimate", "Größe", "labels", "parent", "priority", "Release version", "shell", "sprint", "status",
            "type", "NOT", "IS blocked", "IS favorite", "ORDER BY"
        ]
    );
    assert_eq!(r[0].kind, SuggestionKind::Field);
    assert!(r.iter().all(|s| s.replace_range == (0..0)));
}

#[test]
fn suggest_filters_by_prefix_case_insensitively() {
    assert_eq!(labels(&sug("pri")), vec!["priority"]);
    assert_eq!(labels(&sug("PRI")), vec!["priority"]);
    assert_eq!(labels(&sug("s")), vec!["shell", "sprint", "status", "SORT BY"]);
    assert!(sug("qqq").is_empty());
}

#[test]
fn suggest_quotes_fields_with_spaces() {
    let r = sug("rel");
    assert_eq!(labels(&r), vec!["Release version"]);
    assert_eq!(inserts(&r), vec!["\"Release version\""]);
    assert_eq!(r[0].replace_range, 0..3);
    assert_eq!(r[0].kind, SuggestionKind::Field);
}

#[test]
fn suggest_replaces_a_started_quote() {
    let (input, r) = sug_at("\"Rel|");
    assert_eq!(input, "\"Rel");
    assert_eq!(labels(&r), vec!["Release version"]);
    assert_eq!(r[0].replace_range, 0..4);
    // schon geschlossen und Cursor in der Mitte: der ganze Text samt Anführungszeichen wird ersetzt
    let (input, r) = sug_at("\"Rel|ease version\" = 1");
    assert_eq!(labels(&r), vec!["Release version"]);
    assert_eq!(r[0].replace_range, 0..input.find(" =").unwrap());
}

#[test]
fn suggest_replaces_the_whole_word_around_the_cursor() {
    let (input, r) = sug_at("sta|tus = Done");
    assert_eq!(labels(&r), vec!["status"]);
    assert_eq!(r[0].replace_range, 0..6);
    assert_eq!(&input[r[0].replace_range.clone()], "status");
}

#[test]
fn suggest_operators_after_a_field() {
    let r = sug("status ");
    assert_eq!(labels(&r), vec!["=", "!=", "IN", "NOT IN", "IS EMPTY", "IS NOT EMPTY"]);
    assert!(r.iter().all(|s| s.kind == SuggestionKind::Operator));
    assert_eq!(labels(&sug("estimate ")), vec!["=", "!=", "<", ">", "<=", ">=", "IN", "NOT IN", "IS EMPTY", "IS NOT EMPTY"]);
    let r = sug("\"Release version\" ");
    assert!(labels(&r).contains(&"~") && !labels(&r).contains(&"<"));
    assert_eq!(labels(&sug("archived ")), vec!["=", "!=", "IS EMPTY", "IS NOT EMPTY"]);
    // unbekanntes Feld: alles anbieten
    assert_eq!(sug("nope ").len(), OpKind::ALL.len());
}

#[test]
fn suggest_operator_prefix() {
    assert_eq!(labels(&sug("status i")), vec!["IN", "IS EMPTY", "IS NOT EMPTY"]);
    assert_eq!(labels(&sug("status not")), vec!["NOT IN"]);
    assert_eq!(sug("status i")[0].replace_range, 7..8);
}

#[test]
fn suggest_values_of_an_option_field_in_schema_order() {
    let r = sug("status = ");
    // zuerst die Werte in der Reihenfolge der Werteliste, danach passende Funktionen
    assert_eq!(
        labels(&r),
        vec!["Open", "In Progress", "In Review", "Development finished", "Done", "activeSprint()"]
    );
    assert_eq!(inserts(&r)[..5], ["Open", "\"In Progress\"", "\"In Review\"", "\"Development finished\"", "Done"]);
    assert!(r[..5].iter().all(|s| s.kind == SuggestionKind::Value));
    assert_eq!(r[5].kind, SuggestionKind::Function);
}

#[test]
fn suggest_value_prefix_and_quotes() {
    // `in` ist ein reserviertes Wort und kein Wert: nach „status = in “ gibt es nichts Sinnvolles
    assert!(sug("status = in ").is_empty());
    let r = sug("status = in");
    assert_eq!(labels(&r), vec!["In Progress", "In Review"]);
    assert_eq!(inserts(&r), vec!["\"In Progress\"", "\"In Review\""]);
    assert_eq!(r[0].replace_range, 9..11);
    let r = sug("status = \"In P");
    assert_eq!(labels(&r), vec!["In Progress"]);
    assert_eq!(r[0].replace_range, 9..14);
}

#[test]
fn suggest_adds_a_space_when_glued_to_the_previous_token() {
    assert_eq!(sug("status =")[0].insert_text, " Open");
    assert_eq!(sug("status = ")[0].insert_text, "Open");
    assert!(sug("status IN (Done,")[0].insert_text.starts_with(' '));
    assert_eq!(sug("status IN (")[0].insert_text, "Open");
}

#[test]
fn suggest_in_list_values_and_separators() {
    assert_eq!(sug("status IN (").len(), 6);
    assert_eq!(sug("status IN (Done, ").len(), 6);
    let r = sug("status IN (Done ");
    assert_eq!(labels(&r), vec![",", ")"]);
    assert!(r.iter().all(|s| s.insert_text == s.label), "Satzzeichen hängen ohne Leerzeichen an");
    assert_eq!(labels(&sug("status IN ")), vec!["("]);
    assert_eq!(labels(&sug("status NOT ")), vec!["IN"]);
    assert_eq!(labels(&sug("status NOT IN (Do")), vec!["Done"]);
}

#[test]
fn suggest_functions_as_values() {
    let r = sug("due < ");
    assert_eq!(labels(&r), vec!["currentDate()"]);
    assert_eq!(r[0].kind, SuggestionKind::Function);
    assert_eq!(r[0].insert_text, "currentDate()");
    assert_eq!(labels(&sug("sprint = ")), vec!["Sprint 1", "Sprint 2", "activeSprint()"]);
    assert_eq!(labels(&sug("shell = ")), vec!["linkedTerminal()"]);
    assert_eq!(labels(&sug("sprint = act")), vec!["activeSprint()"]);
    assert!(sug("estimate > ").is_empty());
}

#[test]
fn suggest_bool_values() {
    assert_eq!(labels(&sug("archived = ")), vec!["true", "false"]);
}

#[test]
fn suggest_keywords_after_a_condition() {
    let r = sug("status = Done ");
    assert_eq!(labels(&r), vec!["AND", "OR", "ORDER BY"]);
    assert!(r.iter().all(|s| s.kind == SuggestionKind::Keyword));
    assert_eq!(labels(&sug("status = Done an")), vec!["AND"]);
    assert_eq!(labels(&sug("status = Done o")), vec!["OR", "ORDER BY"]);
    assert_eq!(labels(&sug("status = Done so")), vec!["SORT BY"]);
    assert_eq!(labels(&sug("status IN (Done) ")), vec!["AND", "OR", "ORDER BY"]);
    assert_eq!(labels(&sug("status IS EMPTY ")), vec!["AND", "OR", "ORDER BY"]);
    assert_eq!(labels(&sug("IS favorite ")), vec!["AND", "OR", "ORDER BY"]);
}

#[test]
fn suggest_after_and_or_not_and_open_paren() {
    for s in ["status = Done AND ", "status = Done or ", "NOT ", "(", "(status = Done OR "] {
        let r = sug(s);
        assert!(r.len() >= 14, "{s}: {}", r.len());
        assert_eq!(r[0].label, "archived", "{s}");
    }
    assert_eq!(labels(&sug("status = Done AND pri")), vec!["priority"]);
}

#[test]
fn suggest_close_paren_inside_a_group() {
    let r = sug("(status = Done ");
    assert_eq!(labels(&r), vec!["AND", "OR", "ORDER BY", ")"]);
    assert_eq!(r[3].insert_text, ")");
    assert_eq!(labels(&sug("(status = Done)  ")), vec!["AND", "OR", "ORDER BY"]);
}

#[test]
fn suggest_is_keyword_forms() {
    assert_eq!(labels(&sug("status IS ")), vec!["NOT", "EMPTY", "NULL"]);
    assert_eq!(labels(&sug("status IS NOT ")), vec!["EMPTY", "NULL"]);
    assert_eq!(labels(&sug("status IS e")), vec!["EMPTY"]);
}

#[test]
fn suggest_predicates() {
    let r = sug("IS ");
    assert_eq!(labels(&r), vec!["NOT", "blocked", "favorite"]);
    assert_eq!(r[1].kind, SuggestionKind::Predicate);
    assert_eq!(labels(&sug("IS NOT ")), vec!["blocked", "favorite"]);
    assert_eq!(labels(&sug("IS fa")), vec!["favorite"]);
    // am Anfang einer Bedingung als „IS name“, auch wenn nur der Name getippt wird
    let r = sug("fav");
    assert_eq!(labels(&r), vec!["IS favorite"]);
    assert_eq!(r[0].insert_text, "IS favorite");
    assert_eq!(r[0].kind, SuggestionKind::Predicate);
    assert_eq!(labels(&sug("status = Done AND IS ")), vec!["NOT", "blocked", "favorite"]);
}

#[test]
fn suggest_order_by() {
    assert_eq!(labels(&sug("status = Done ORDER ")), vec!["BY"]);
    assert_eq!(labels(&sug("status = Done order b")), vec!["BY"]);
    let r = sug("ORDER BY ");
    let l = labels(&r);
    assert!(l.contains(&"priority") && l.contains(&"Release version"));
    assert!(!l.contains(&"labels") && !l.contains(&"components"), "Mehrfachwert-Felder lassen sich nicht sortieren");
    assert_eq!(*l.last().unwrap(), "favorites");
    assert_eq!(r.last().unwrap().kind, SuggestionKind::SortKey);
    assert_eq!(labels(&sug("ORDER BY fav")), vec!["favorites"]);
    assert_eq!(labels(&sug("ORDER BY priority ")), vec!["ASC", "DESC", ","]);
    assert_eq!(labels(&sug("ORDER BY priority DESC ")), vec![","]);
    assert_eq!(labels(&sug("ORDER BY priority DESC, pr")), vec!["priority"]);
    assert_eq!(sug("ORDER BY priority, ").len(), sug("ORDER BY ").len());
    assert_eq!(labels(&sug("SORT BY fa")), vec!["favorites"]);
}

#[test]
fn suggest_order_by_at_the_start_of_the_query() {
    assert_eq!(labels(&sug("ord")), vec!["ORDER BY"]);
    assert_eq!(labels(&sug("sor")), vec!["SORT BY"]);
}

#[test]
fn suggest_with_unicode_fields() {
    let (input, r) = sug_at("grö|");
    assert_eq!(labels(&r), vec!["Größe"]);
    assert_eq!(input.len(), 4); // ö ist zwei Bytes
    assert_eq!(r[0].replace_range, 0..input.len());
    let (input, r) = sug_at("Größe > 3 AND Gr|");
    assert_eq!(labels(&r), vec!["Größe"]);
    assert_eq!(&input[r[0].replace_range.clone()], "Gr");
}

#[test]
fn suggest_cursor_in_the_middle_of_a_multibyte_char_is_clamped() {
    let input = "Größe > 3";
    // Byte 3 liegt mitten im „ö“ (Bytes 2..4): wird auf die Zeichengrenze davor gezogen
    let r = suggest(input, 3, &test_schema());
    assert_eq!(labels(&r), vec!["Größe"]);
    // Cursor hinter dem Ende
    assert_eq!(suggest("sta", 999, &test_schema()).len(), 1);
}

#[test]
fn suggest_looks_only_left_of_the_cursor() {
    let (_, r) = sug_at("status = Done AND |priority = High");
    assert_eq!(r[0].label, "archived");
    assert_eq!(r[0].replace_range, 18..18);
    let (_, r) = sug_at("sta|tus = Done");
    assert_eq!(labels(&r), vec!["status"]);
    let (_, r) = sug_at("|status = Done");
    assert_eq!(r[0].replace_range, 0..0);
    let (_, r) = sug_at("status |= Done");
    assert_eq!(labels(&r), vec!["=", "!=", "IN", "NOT IN", "IS EMPTY", "IS NOT EMPTY"]);
}

#[test]
fn suggest_gives_nothing_in_unclear_places() {
    assert!(sug("status = Done Done ").is_empty());
    assert!(sug("status @ ").is_empty());
    assert!(sug("due = currentDate(").is_empty());
    assert!(sug("status = Done ORDER BY priority DESC DESC ").is_empty());
    // nach einem Fehler geht es hinter dem nächsten AND wieder los
    assert_eq!(sug("status = Done Done AND pri").len(), 1);
}

#[test]
fn suggest_after_a_function_call() {
    assert_eq!(labels(&sug("due < currentDate() ")), vec!["AND", "OR", "ORDER BY"]);
    assert_eq!(labels(&sug("shell IN (linkedTerminal(), ")), vec!["linkedTerminal()"]);
    assert_eq!(labels(&sug("shell IN (linkedTerminal() ")), vec![",", ")"]);
}

#[test]
fn suggest_with_a_custom_schema() {
    let schema = Schema::new()
        .with_field("Größe der Datei", FieldType::Number)
        .with_field("Typ", FieldType::Option(vec!["Café".into(), "a b".into(), "x\"y".into(), "ok".into()]))
        .with_predicate("mit Anhang");
    let r = suggest("", 0, &schema);
    assert_eq!(labels(&r), vec!["Größe der Datei", "Typ", "NOT", "IS mit Anhang", "ORDER BY"]);
    assert_eq!(inserts(&r), vec!["\"Größe der Datei\"", "Typ", "NOT", "IS \"mit Anhang\"", "ORDER BY"]);
    let r = suggest("Typ = ", 6, &schema);
    assert_eq!(inserts(&r)[..4], ["Café", "\"a b\"", "\"x\\\"y\"", "ok"]);
}

#[test]
fn suggested_values_form_valid_text_when_inserted() {
    // Jeder Wert-Vorschlag, eingesetzt, ergibt eine lesbare und gültige Abfrage.
    let base = "status = ";
    for s in sug(base) {
        let text = format!("{}{}", &base[..s.replace_range.start], s.insert_text);
        let query = parse(&text).unwrap_or_else(|e| panic!("{text:?}: {e:?}"));
        assert!(validate(&query, &test_schema()).is_empty(), "{text:?}");
    }
}

#[test]
fn suggest_results_are_unique() {
    for s in ["", "status ", "ORDER BY ", "status = ", "status = Done "] {
        let r = sug(s);
        for (i, a) in r.iter().enumerate() {
            assert!(!r[i + 1..].iter().any(|b| b.label == a.label && b.kind == a.kind), "{s}: doppelt {}", a.label);
        }
    }
}
