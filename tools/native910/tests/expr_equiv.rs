//! Expression-statement equivalence gate: every hand-written `$y = <expr>;`
//! source assembles byte-identical to its hand-written flat
//! push/operate/pop equivalent, reparses stably, and every documented
//! rejection fails loudly.
//!
//! Flat equivalents use the canonical spellings the lowerer emits
//! (`push_constant_string` family for literals, typed `push_*_local` for
//! `$locals`, `op(0)` for operators) — so any drift in emission order,
//! operand choice, or slot resolution shows up as a byte mismatch.

mod common;

use native910::config::ConfigTypes;
use native910::expr::Expr;
use native910::inames::InterfaceRegistry;
use native910::opcode::OpcodeBook;
use native910::parse::parse_source;
use native910::script::decode_script;
use native910::source::{
    Param, SourceScript, SourceStmt, ValType, assemble_source, format_source, lower,
};
use native910::symbols::SymbolRegistry;

fn book() -> OpcodeBook {
    OpcodeBook::embedded().unwrap()
}

fn symbols() -> SymbolRegistry {
    SymbolRegistry::empty()
}

fn inames() -> InterfaceRegistry {
    InterfaceRegistry::empty()
}

fn configs() -> ConfigTypes {
    ConfigTypes::empty()
}

/// Assemble `body` lines under `decls` with an anonymous header and a
/// trailing `return`, panicking with the full text on failure.
fn assemble_body(decls: &str, body: &str) -> Vec<u8> {
    let text = format!("[]()\n{decls}\n{body}    return(0);\n");
    assemble_source(&text, &book(), &symbols(), &configs(), &inames())
        .unwrap_or_else(|error| panic!("assemble failed for:\n{text}\n{error}"))
}

/// `$stmt` must assemble byte-identical to the hand-written flat `flat`
/// sequence, and must survive a parse-format-reparse fixpoint.
fn assert_equiv(decls: &str, stmt: &str, flat: &str) {
    let expr_bytes = assemble_body(decls, &format!("    {stmt}\n"));
    let flat_bytes = assemble_body(decls, flat);
    assert_eq!(
        expr_bytes, flat_bytes,
        "expression `{stmt}` diverged from its flat equivalent"
    );
    let text = format!("[]()\n{decls}\n    {stmt}\n    return(0);\n");
    let first =
        parse_source(&text, &symbols(), &configs(), &inames()).expect("parse expression source");
    let formatted = format_source(&first, &symbols(), &inames()).expect("format expression source");
    let second = parse_source(&formatted, &symbols(), &configs(), &inames())
        .expect("reparse formatted source");
    // Bodies must fixpoint exactly; header/locals are excluded because the
    // formatter canonically regroups declarations by type (pre-existing
    // behavior, orthogonal to expressions).
    assert_eq!(
        first.body, second.body,
        "parse∘format fixpoint broke for `{stmt}`"
    );
}

/// `stmt` must fail with an error containing `fragment`.
fn assert_rejects(decls: &str, stmt: &str, fragment: &str) {
    let text = format!("[]()\n{decls}\n    {stmt}\n    return(0);\n");
    let error = assemble_source(&text, &book(), &symbols(), &configs(), &inames())
        .expect_err(&format!("accepted but must reject: {stmt}"));
    assert!(
        error.to_string().contains(fragment),
        "error `{error}` misses `{fragment}` for `{stmt}`"
    );
}

const INTS: &str = "int $a, $b, $c, $y;\n";

#[test]
fn infix_operators_match_flat_sequences() {
    assert_equiv(
        INTS,
        "$y = 1 + 2;",
        "    push_constant_string(1);\n    push_constant_string(2);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a + 41;",
        "    push_int_local($a);\n    push_constant_string(41);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = 41 + $a;",
        "    push_constant_string(41);\n    push_int_local($a);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a + $b;",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a * $b;",
        "    push_int_local($a);\n    push_int_local($b);\n    multiply(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a / $b;",
        "    push_int_local($a);\n    push_int_local($b);\n    divide(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a % $b;",
        "    push_int_local($a);\n    push_int_local($b);\n    modulo(0);\n    pop_int_local($y);\n",
    );
    // Non-commutative order is left-first: `$a / $b`, not `$b / $a`.
    assert_equiv(
        INTS,
        "$y = $b / $a;",
        "    push_int_local($b);\n    push_int_local($a);\n    divide(0);\n    pop_int_local($y);\n",
    );
}

#[test]
fn precedence_and_parens_lower_in_evaluation_order() {
    // `*` binds tighter: a, then (b*c), then add.
    assert_equiv(
        INTS,
        "$y = $a + $b * $c;",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    multiply(0);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = ($a + $b) * $c;",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    push_int_local($c);\n    multiply(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a * ($b + $c);",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    add(0);\n    multiply(0);\n    pop_int_local($y);\n",
    );
    // Left-assoc chains evaluate left to right.
    assert_equiv(
        INTS,
        "$y = $a + $b + $c;",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    push_int_local($c);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = $a % $b + $c / $a;",
        "    push_int_local($a);\n    push_int_local($b);\n    modulo(0);\n    push_int_local($c);\n    push_int_local($a);\n    divide(0);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = ($a);",
        "    push_int_local($a);\n    pop_int_local($y);\n",
    );
    // Unspaced surface still parses and canonicalizes stably.
    assert_equiv(
        INTS,
        "$y=$a+($b*$c);",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    multiply(0);\n    add(0);\n    pop_int_local($y);\n",
    );
    // Negative literals are values, not operations.
    assert_equiv(
        INTS,
        "$y = -5 + $a;",
        "    push_constant_string(-5);\n    push_int_local($a);\n    add(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = -2147483648;",
        "    push_constant_string(-2147483648);\n    pop_int_local($y);\n",
    );
}

#[test]
fn function_operators_match_flat_sequences() {
    for (op, command) in [("min", "min"), ("max", "max"), ("and", "and"), ("or", "or")] {
        assert_equiv(
            INTS,
            &format!("$y = {op}($a, $b);"),
            &format!(
                "    push_int_local($a);\n    push_int_local($b);\n    {command}(0);\n    pop_int_local($y);\n"
            ),
        );
        assert_equiv(
            INTS,
            &format!("$y = {op}(1, 2);"),
            &format!(
                "    push_constant_string(1);\n    push_constant_string(2);\n    {command}(0);\n    pop_int_local($y);\n"
            ),
        );
    }
    assert_equiv(
        INTS,
        "$y = min($a + $b, $c * 2);",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    push_int_local($c);\n    push_constant_string(2);\n    multiply(0);\n    min(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = min(max($a, $b), $c + 1);",
        "    push_int_local($a);\n    push_int_local($b);\n    max(0);\n    push_int_local($c);\n    push_constant_string(1);\n    add(0);\n    min(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = and(or($a, $b), $c);",
        "    push_int_local($a);\n    push_int_local($b);\n    or(0);\n    push_int_local($c);\n    and(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = min($a + $b * 2, not($c));",
        "    push_int_local($a);\n    push_int_local($b);\n    push_constant_string(2);\n    multiply(0);\n    add(0);\n    push_int_local($c);\n    not(0);\n    min(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = not($a);",
        "    push_int_local($a);\n    not(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = not(41);",
        "    push_constant_string(41);\n    not(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = not($a + $b);",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    not(0);\n    pop_int_local($y);\n",
    );
}

#[test]
fn pure_int_operators_match_flat_sequences() {
    // Every new homogeneous int operator lowers left-first with a zero
    // generic operand, exactly like the original `min`/`max` family.
    for (op, command) in [
        ("addpercent", "addpercent"),
        ("setbit", "setbit"),
        ("clearbit", "clearbit"),
        ("testbit", "testbit"),
        ("pow", "pow"),
        (
            "quickchat_dynamic_command_add",
            "quickchat_dynamic_command_add",
        ),
    ] {
        assert_equiv(
            INTS,
            &format!("$y = {op}($a, $b);"),
            &format!(
                "    push_int_local($a);\n    push_int_local($b);\n    {command}(0);\n    pop_int_local($y);\n"
            ),
        );
        assert_equiv(
            INTS,
            &format!("$y = {op}(1, 2);"),
            &format!(
                "    push_constant_string(1);\n    push_constant_string(2);\n    {command}(0);\n    pop_int_local($y);\n"
            ),
        );
    }
    // Non-commutative order is left-first here too.
    assert_equiv(
        INTS,
        "$y = quickchat_dynamic_command_add($b, $a);",
        "    push_int_local($b);\n    push_int_local($a);\n    quickchat_dynamic_command_add(0);\n    pop_int_local($y);\n",
    );
    // New operators nest with old ones in evaluation order.
    assert_equiv(
        INTS,
        "$y = setbit($a + $b, testbit($c, 1));",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    push_int_local($c);\n    push_constant_string(1);\n    testbit(0);\n    setbit(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = pow(addpercent($a, $b), 2);",
        "    push_int_local($a);\n    push_int_local($b);\n    addpercent(0);\n    push_constant_string(2);\n    pow(0);\n    pop_int_local($y);\n",
    );
    // Unary int producers.
    assert_equiv(
        INTS,
        "$y = random($a);",
        "    push_int_local($a);\n    random(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = randominc(6);",
        "    push_constant_string(6);\n    randominc(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = random($a + $b);",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    random(0);\n    pop_int_local($y);\n",
    );
}

#[test]
fn nary_and_tostring_operators_match_flat_sequences() {
    // `scale` lowers left-first with a zero generic operand, exactly like the
    // binary int family, with three pushes instead of two.
    assert_equiv(
        INTS,
        "$y = scale($a, $b, $c);",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    scale(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = scale(1, 2, 3);",
        "    push_constant_string(1);\n    push_constant_string(2);\n    push_constant_string(3);\n    scale(0);\n    pop_int_local($y);\n",
    );
    // `interpolate` is the same machinery with five pushes.
    assert_equiv(
        INTS,
        "$y = interpolate($a, $b, $c, $a, $b);",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    push_int_local($a);\n    push_int_local($b);\n    interpolate(0);\n    pop_int_local($y);\n",
    );
    // N-ary operators nest with old ones in evaluation order.
    assert_equiv(
        INTS,
        "$y = scale($a + $b, min($c, 1), 2);",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    push_int_local($c);\n    push_constant_string(1);\n    min(0);\n    push_constant_string(2);\n    scale(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = min(scale($a, $b, $c), $a);",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    scale(0);\n    push_int_local($a);\n    min(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = scale($a, $b, $c) + 1;",
        "    push_int_local($a);\n    push_int_local($b);\n    push_int_local($c);\n    scale(0);\n    push_constant_string(1);\n    add(0);\n    pop_int_local($y);\n",
    );
    // `tostring` is the int->string unary row: one int push, one object out.
    const MIXED: &str = "string $s;\nint $a, $y;\n";
    assert_equiv(
        MIXED,
        "$s = tostring($a);",
        "    push_int_local($a);\n    tostring(0);\n    pop_string_local($s);\n",
    );
    assert_equiv(
        MIXED,
        "$s = tostring(41);",
        "    push_constant_string(41);\n    tostring(0);\n    pop_string_local($s);\n",
    );
    assert_equiv(
        MIXED,
        "$s = tostring(scale($a, $y, 100));",
        "    push_int_local($a);\n    push_int_local($y);\n    push_constant_string(100);\n    scale(0);\n    tostring(0);\n    pop_string_local($s);\n",
    );
    assert_equiv(
        MIXED,
        "$y = string_length(tostring($a));",
        "    push_int_local($a);\n    tostring(0);\n    string_length(0);\n    pop_int_local($y);\n",
    );
}

#[test]
fn string_concat_operator_matches_flat_sequence() {
    const STR: &str = "string $s, $t, $u;\nint $y;\n";
    // `append` is the first string-producing operator: two object pushes,
    // one object push out, popped into a string slot.
    assert_equiv(
        STR,
        "$u = append($s, $t);",
        "    push_string_local($s);\n    push_string_local($t);\n    append(0);\n    pop_string_local($u);\n",
    );
    assert_equiv(
        STR,
        "$u = append(\"a\", $s);",
        "    push_constant_string(\"a\");\n    push_string_local($s);\n    append(0);\n    pop_string_local($u);\n",
    );
    assert_equiv(
        STR,
        "$y = string_length(append($s, $t));",
        "    push_string_local($s);\n    push_string_local($t);\n    append(0);\n    string_length(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        STR,
        "$y = compare(append($s, \"x\"), $t);",
        "    push_string_local($s);\n    push_constant_string(\"x\");\n    append(0);\n    push_string_local($t);\n    compare(0);\n    pop_int_local($y);\n",
    );
}

#[test]
fn join_string_matches_flat_sequences() {
    const STRS: &str = "string $s, $t, $u;\n";
    // Counts 2, 3, 5: each part lowers left-first, then the command with its
    // explicit count operand (never a zero generic) — the same bytes as the
    // hand-written flat `join_string(N)` form.
    assert_equiv(
        STRS,
        "$u = join_string($s, $t);",
        "    push_string_local($s);\n    push_string_local($t);\n    join_string(2);\n    pop_string_local($u);\n",
    );
    assert_equiv(
        STRS,
        "$u = join_string(\"a\", $s, $t);",
        "    push_constant_string(\"a\");\n    push_string_local($s);\n    push_string_local($t);\n    join_string(3);\n    pop_string_local($u);\n",
    );
    assert_equiv(
        STRS,
        "$u = join_string($s, $t, \"x\", \"y\", $s);",
        "    push_string_local($s);\n    push_string_local($t);\n    push_constant_string(\"x\");\n    push_constant_string(\"y\");\n    push_string_local($s);\n    join_string(5);\n    pop_string_local($u);\n",
    );
    // The goal form: string literals and `$locals` mix freely.
    assert_equiv(
        STRS,
        "$u = join_string(\"a\", $s, \"!\");",
        "    push_constant_string(\"a\");\n    push_string_local($s);\n    push_constant_string(\"!\");\n    join_string(3);\n    pop_string_local($u);\n",
    );
    // Commas inside string parts are data, not argument separators.
    assert_equiv(
        STRS,
        "$u = join_string(\"a, b\", $s);",
        "    push_constant_string(\"a, b\");\n    push_string_local($s);\n    join_string(2);\n    pop_string_local($u);\n",
    );
    // Nesting: `append` feeds `join`, `join` feeds `string_length`/`compare`.
    const MIXED: &str = "string $s, $t, $u;\nint $y;\n";
    assert_equiv(
        MIXED,
        "$u = join_string(append($s, $t), \"!\");",
        "    push_string_local($s);\n    push_string_local($t);\n    append(0);\n    push_constant_string(\"!\");\n    join_string(2);\n    pop_string_local($u);\n",
    );
    assert_equiv(
        MIXED,
        "$y = string_length(join_string($s, $t, $u));",
        "    push_string_local($s);\n    push_string_local($t);\n    push_string_local($u);\n    join_string(3);\n    string_length(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        MIXED,
        "$y = compare(join_string($s, \"x\"), $t);",
        "    push_string_local($s);\n    push_constant_string(\"x\");\n    join_string(2);\n    push_string_local($t);\n    compare(0);\n    pop_int_local($y);\n",
    );
}

#[test]
fn join_string_rejections_are_loud() {
    const STRS: &str = "string $s, $t, $u;\n";
    // Degenerate counts: the written count is the arity, and 0/1 have no form.
    assert_rejects(STRS, "$u = join_string($s);", "takes at least 2 arguments");
    assert_rejects(STRS, "$u = join_string();", "takes at least 2 arguments");
    // Int parts rejected; the string result cannot land in an int slot.
    assert_rejects(STRS, "$u = join_string($s, 1);", "needs string operands");
    assert_rejects(STRS, "$u = join_string(1, 2);", "needs string operands");
    assert_rejects(
        STRS,
        "$u = join_string($s, $t, 3);",
        "needs string operands",
    );
    assert_rejects(
        "string $s, $t;\nint $y;\n",
        "$y = join_string($s, $t);",
        "cannot assign string expression to int",
    );
    // Adjacent misspellings still fail as unknown operators.
    assert_rejects(
        STRS,
        "$u = joinstring($s, $t);",
        "unknown operator 'joinstring'",
    );
}

#[test]
fn string_and_long_combinations_match_flat() {
    const STR: &str = "string $s, $t;\nint $y;\n";
    assert_equiv(
        STR,
        "$y = compare($s, $t);",
        "    push_string_local($s);\n    push_string_local($t);\n    compare(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        STR,
        "$y = compare(\"a\", \"b\");",
        "    push_constant_string(\"a\");\n    push_constant_string(\"b\");\n    compare(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        STR,
        "$y = compare($s, \"done\");",
        "    push_string_local($s);\n    push_constant_string(\"done\");\n    compare(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        STR,
        "$y = string_length($s);",
        "    push_string_local($s);\n    string_length(0);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        STR,
        "$y = string_length(\"hi\");",
        "    push_constant_string(\"hi\");\n    string_length(0);\n    pop_int_local($y);\n",
    );
    // Commas inside strings are data, not argument separators.
    assert_equiv(
        STR,
        "$y = string_length(\"a, b\");",
        "    push_constant_string(\"a, b\");\n    string_length(0);\n    pop_int_local($y);\n",
    );
    // String passthrough: no string-producing operator, just the slot move.
    assert_equiv(
        STR,
        "$t = $s;",
        "    push_string_local($s);\n    pop_string_local($t);\n",
    );
    assert_equiv(
        STR,
        "$t = \"x\";",
        "    push_constant_string(\"x\");\n    pop_string_local($t);\n",
    );
    // Escapes and comment markers inside strings survive the round trip.
    assert_equiv(
        STR,
        "$t = \"say \\\"hi\\\" // ok\";",
        "    push_constant_string(\"say \\\"hi\\\" // ok\");\n    pop_string_local($t);\n",
    );

    const LONG: &str = "long $l, $m;\n";
    assert_equiv(
        LONG,
        "$m = $l;",
        "    push_long_local($l);\n    pop_long_local($m);\n",
    );
    assert_equiv(
        LONG,
        "$m = 99L;",
        "    push_constant_string(99L);\n    pop_long_local($m);\n",
    );
    assert_equiv(
        LONG,
        "$m = -7L;",
        "    push_constant_string(-7L);\n    pop_long_local($m);\n",
    );
    assert_equiv(
        LONG,
        "$m = -9223372036854775808L;",
        "    push_constant_string(-9223372036854775808L);\n    pop_long_local($m);\n",
    );

    assert_equiv(
        INTS,
        "$y = $a;",
        "    push_int_local($a);\n    pop_int_local($y);\n",
    );
    assert_equiv(
        INTS,
        "$y = 7;",
        "    push_constant_string(7);\n    pop_int_local($y);\n",
    );
}

#[test]
fn assignments_read_args_and_respect_labels() {
    // `$in` is an argument slot, not a local — same push, slot 0.
    let expr_text = "[p](int $in)\nint $y;\n\n    $y = $in * 2 + 1;\n    return(0);\n";
    let flat_text = "[p](int $in)\nint $y;\n\n    push_int_local($in);\n    push_constant_string(2);\n    multiply(0);\n    push_constant_string(1);\n    add(0);\n    pop_int_local($y);\n    return(0);\n";
    assert_eq!(
        assemble_source(expr_text, &book(), &symbols(), &configs(), &inames()).unwrap(),
        assemble_source(flat_text, &book(), &symbols(), &configs(), &inames()).unwrap()
    );
    // Labels never see the assignment: a branch over it indexes exactly the
    // flat instructions.
    assert_equiv(
        "int $y;\n",
        "$y = 1 + 2;",
        "    push_constant_string(1);\n    push_constant_string(2);\n    add(0);\n    pop_int_local($y);\n",
    );
    let branched_expr =
        "[]()\nint $y;\n\n    branch(done);\n    $y = 1 + 2;\ndone:\n    return(0);\n";
    let branched_flat = "[]()\nint $y;\n\n    branch(done);\n    push_constant_string(1);\n    push_constant_string(2);\n    add(0);\n    pop_int_local($y);\ndone:\n    return(0);\n";
    assert_eq!(
        assemble_source(branched_expr, &book(), &symbols(), &configs(), &inames()).unwrap(),
        assemble_source(branched_flat, &book(), &symbols(), &configs(), &inames()).unwrap()
    );
}

#[test]
fn flagship_sequence_decodes_to_exact_commands() {
    let bytes = assemble_body(INTS, "    $y = $a + 41;\n");
    let decoded = decode_script(&bytes, &book()).unwrap();
    let commands: Vec<&str> = decoded
        .code
        .iter()
        .map(|instr| instr.command.as_str())
        .collect();
    assert_eq!(
        commands,
        vec![
            "push_int_local",
            "push_constant_string",
            "add",
            "pop_int_local",
            "return"
        ]
    );
    assert!(matches!(
        decoded.code[0].operand,
        native910::script::Operand::Local(0)
    ));
    assert!(matches!(
        decoded.code[1].operand,
        native910::script::Operand::Int(41)
    ));
    // INTS declares $a, $b, $c, $y in order: $a is int slot 0, $y is slot 3.
    assert!(matches!(
        decoded.code[3].operand,
        native910::script::Operand::Local(3)
    ));
}

#[test]
fn failures_name_their_lines() {
    let text = "[]()\nint $a, $y;\n\n    push_constant_int(1);\n    $ghost = 1;\n    $y = \"s\";\n";
    let error = parse_source(text, &symbols(), &configs(), &inames()).unwrap_err();
    assert_eq!(error.line, 5);
    let error = parse_source(
        "[]()\nint $a, $y;\n\n    $y = $a + 1;\n    $y = \"s\";\n",
        &symbols(),
        &configs(),
        &inames(),
    )
    .unwrap_err();
    assert_eq!(error.line, 5);
    let error = parse_source(
        "[]()\nint $y;\n\n    $y = 1\n    return(0);\n",
        &symbols(),
        &configs(),
        &inames(),
    )
    .unwrap_err();
    assert_eq!(error.line, 4);
}

#[test]
fn every_documented_rejection_errors() {
    assert_rejects(INTS, "$ghost = 1;", "undeclared local '$ghost'");
    assert_rejects(INTS, "$y = $ghost + 1;", "undeclared local '$ghost'");
    assert_rejects(
        INTS,
        "$y = \"s\";",
        "cannot assign string expression to int '$y'",
    );
    assert_rejects(
        "string $s;\n",
        "$s = 1;",
        "cannot assign int expression to string '$s'",
    );
    assert_rejects(
        "long $m;\n",
        "$m = 1;",
        "cannot assign int expression to long '$m'",
    );
    assert_rejects(INTS, "$y = min(\"a\", \"b\");", "needs (int, int) operands");
    assert_rejects(
        INTS,
        "$y = compare(1, 2);",
        "needs (string, string) operands",
    );
    assert_rejects(INTS, "$y = not(\"s\");", "needs a int operand");
    assert_rejects(INTS, "$y = string_length(5);", "needs a string operand");
    // Calls as values: unknown names fail at typing with a line error...
    assert_rejects(INTS, "$y = ~callee();", "unknown script '~callee'");
    assert_rejects(INTS, "$y = min(~callee(), 2);", "unknown script '~callee'");
    // ...while known names resolve through the registry: Unknown arity,
    // void, and multi-value returns all fail loudly.
    assert_rejects(INTS, "$y = ~novalue();", "unknown script '~novalue'");
    {
        use std::collections::BTreeMap;
        let mut known = BTreeMap::new();
        known.insert(
            700,
            native910::script::Counts {
                int: 0,
                obj: 0,
                long: 0,
            },
        );
        known.insert(
            701,
            native910::script::Counts {
                int: 1,
                obj: 1,
                long: 0,
            },
        );
        let symbols = SymbolRegistry::build(
            vec![
                (700, "voidproc".to_string()),
                (701, "twovalues".to_string()),
            ],
            &known,
        )
        .unwrap()
        .with_returns(&std::collections::BTreeMap::from([(
            701,
            native910::script::Counts {
                int: 1,
                obj: 1,
                long: 0,
            },
        )]));
        let text = |stmt: &str| format!("[]()\n{INTS}\n    {stmt}\n    return(0);\n");
        // Unknown arity (700 has no inference entry) fails as unknowable...
        let error =
            parse_source(&text("$y = ~voidproc();"), &symbols, &configs(), &inames()).unwrap_err();
        assert!(
            error.to_string().contains("statically unknowable"),
            "unexpected error: {error}"
        );
        // ...while known multi-value arity fails on the single-value rule
        // (arity first: two args for the (1i,1o) signature).
        let error = parse_source(
            &text("$y = ~twovalues(1, \"s\");"),
            &symbols,
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("exactly one value required"),
            "unexpected error: {error}"
        );
        // A known single-value callee still checks its arguments: arity and
        // lane against the signature, declared locals, and call syntax.
        known.insert(
            702,
            native910::script::Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
        );
        let one_value = SymbolRegistry::build(vec![(702, "onevalue".to_string())], &known)
            .unwrap()
            .with_returns(&std::collections::BTreeMap::from([(
                702,
                native910::script::Counts {
                    int: 1,
                    obj: 0,
                    long: 0,
                },
            )]));
        for (stmt, fragment) in [
            ("$y = ~onevalue();", "expects (1i,0o,0l), got (0i,0o,0l)"),
            (
                "$y = ~onevalue(\"s\");",
                "expects (1i,0o,0l), got (0i,1o,0l)",
            ),
            ("$y = ~onevalue($ghost);", "undeclared local '$ghost'"),
            ("$y = ~onevalue(1;", "expected ',' or ')'"),
            ("$y = ~123();", "expected a script name after '~'"),
        ] {
            let error = parse_source(&text(stmt), &one_value, &configs(), &inames()).unwrap_err();
            assert!(
                error.to_string().contains(fragment),
                "error `{error}` misses `{fragment}` for `{stmt}`"
            );
        }
        // A known single-int return assembles: pushes, resolved gosub, pop.
        let bytes = assemble_source(
            &text("$y = ~onevalue(41);"),
            &book(),
            &one_value,
            &configs(),
            &inames(),
        )
        .expect("assemble value call");
        let decoded = decode_script(&bytes, &book()).expect("decode assembled");
        assert_eq!(decoded.code.len(), 4);
    }
    // Unverified operators, including real-but-out-of-scope commands.
    assert_rejects(
        INTS,
        "$y = join_string(2);",
        "takes at least 2 arguments, got 1",
    );
    assert_rejects(
        INTS,
        "$y = subtract($a, $b);",
        "unknown operator 'subtract'",
    );
    assert_rejects(INTS, "$y = struct_param($a, $b);", "literal param id");
    assert_rejects(
        INTS,
        "$y = struct_param($a, 999999);",
        "unknown param id 999999",
    );
    assert_rejects(INTS, "$y = _enum($a, $b, $c, $a);", "literal enum id");
    assert_rejects(INTS, "$y = frobnicate(1);", "unknown operator 'frobnicate'");
    // Arity slips.
    assert_rejects(INTS, "$y = min(1);", "takes 2 arguments, got 1");
    assert_rejects(INTS, "$y = min(1, 2, 3);", "takes 2 arguments, got 3");
    assert_rejects(INTS, "$y = not();", "takes 1 argument, got 0");
    assert_rejects(INTS, "$y = not(1, 2);", "takes 1 argument, got 2");
    assert_rejects(INTS, "$y = setbit(1);", "takes 2 arguments, got 1");
    assert_rejects(INTS, "$y = pow(1, 2, 3);", "takes 2 arguments, got 3");
    assert_rejects(INTS, "$y = scale($a, $b);", "takes 3 arguments, got 2");
    assert_rejects(
        INTS,
        "$y = scale($a, $b, $c, $a);",
        "takes 3 arguments, got 4",
    );
    assert_rejects(
        INTS,
        "$y = interpolate($a, $b, $c, $a);",
        "takes 5 arguments, got 4",
    );
    assert_rejects(
        INTS,
        "$y = interpolate($a, $b, $c, $a, $b, $c);",
        "takes 5 arguments, got 6",
    );
    assert_rejects(INTS, "$y = random();", "takes 1 argument, got 0");
    assert_rejects(INTS, "$y = random(1, 2);", "takes 1 argument, got 2");
    assert_rejects(INTS, "$y = tostring();", "takes 1 argument, got 0");
    assert_rejects(INTS, "$y = tostring($a, $b);", "takes 1 argument, got 2");
    assert_rejects(INTS, "$y = append($s);", "takes 2 arguments, got 1");
    // Lane slips: int binaries reject strings, `append` rejects ints, and
    // the string result cannot land in an int slot.
    assert_rejects(INTS, "$y = setbit($a, \"s\");", "needs (int, int) operands");
    assert_rejects(INTS, "$y = pow(\"a\", \"b\");", "needs (int, int) operands");
    assert_rejects(INTS, "$y = random(\"s\");", "needs a int operand");
    assert_rejects(
        INTS,
        "$y = scale($a, $b, \"s\");",
        "needs (int, int, int) operands",
    );
    assert_rejects(
        INTS,
        "$y = interpolate($a, $b, $c, $a, \"s\");",
        "needs (int, int, int, int, int) operands",
    );
    assert_rejects(
        INTS,
        "$y = tostring($a);",
        "cannot assign string expression to int",
    );
    const STR3: &str = "string $s;\nint $y;\n";
    assert_rejects(STR3, "$y = tostring($s);", "needs a int operand");
    const STR2: &str = "string $s, $t;\nint $y;\n";
    assert_rejects(
        STR2,
        "$s = append($s, 1);",
        "needs (string, string) operands",
    );
    assert_rejects(
        STR2,
        "$y = append($s, $t);",
        "cannot assign string expression to int",
    );
    // Bare computed values and malformed assignments.
    assert_rejects(INTS, "$a + $b;", "malformed assignment");
    assert_rejects(INTS, "min($a, $b);", "bad int operand");
    assert_rejects(INTS, "$y = 1", "misses its ';'");
    assert_rejects(INTS, "$y =;", "empty expression");
    assert_rejects(INTS, "$y = $a == 1;", "unexpected character '='");
    // Expression syntax: bare operator words, infix/unary minus (no
    // subtract/negate commands), unbalanced or dangling forms, bad string
    // literals, and the nesting depth limit.
    assert_rejects(INTS, "$y = min;", "unexpected name 'min'");
    assert_rejects(INTS, "$y = $a - $b;", "unexpected '-' after expression");
    assert_rejects(INTS, "$y = -$a;", "applies to number literals only");
    assert_rejects(INTS, "$y = ($a + $b;", "missing ')'");
    assert_rejects(INTS, "$y = $a +;", "unexpected end of expression");
    assert_rejects("string $s;\n", "$s = \"abc;", "unterminated string literal");
    assert_rejects("string $s;\n", "$s = \"\\q\";", "bad escape");
    assert_rejects(
        INTS,
        &format!("$y = {}1;", "(".repeat(80)),
        "too deeply nested",
    );
    assert_rejects(INTS, "$y = addpercent(1);", "takes 2 arguments, got 1");
    assert_rejects(INTS, "$y = setbit(1, 2, 3);", "takes 2 arguments, got 3");
}

#[test]
fn lower_revalidates_programmatic_models_without_lines() {
    fn typed_model(ty: ValType, target: &str, expr: Expr) -> SourceScript {
        SourceScript {
            name: None,
            args: Vec::new(),
            locals: vec![Param {
                ty,
                name: "y".to_string(),
            }],
            body: vec![SourceStmt::Assign {
                target: target.to_string(),
                expr,
            }],
        }
    }
    fn model(target: &str, expr: Expr) -> SourceScript {
        typed_model(ValType::Int, target, expr)
    }
    let book = book();
    let symbols = symbols();
    // Undeclared target fails in lowering, not just in parsing.
    let error = lower(
        &model("ghost", Expr::LitInt(1)),
        &book,
        &symbols,
        &configs(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("undeclared local '$ghost'"), "got: {error}");
    // Type mismatch fails in lowering too (the parser would have caught the
    // same model first, but programmatic models skip the parser).
    let error = lower(
        &model("y", Expr::LitStr("s".to_string())),
        &book,
        &symbols,
        &configs(),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("cannot assign string expression"),
        "got: {error}"
    );
    // Shapes the parser can never produce are re-checked in lowering too:
    // degenerate join counts, n-ary arity slips, unpackable component pairs.
    for (case, ty, expr) in [
        (
            "one-part join",
            ValType::String,
            Expr::Join(vec![Expr::LitStr("s".to_string())]),
        ),
        ("empty join", ValType::String, Expr::Join(Vec::new())),
        (
            "one-arg scale",
            ValType::Int,
            Expr::Nary(native910::expr::NaryOp::Scale, vec![Expr::LitInt(1)]),
        ),
        (
            "unpackable component",
            ValType::Int,
            Expr::Component {
                iface: i32::MAX,
                child: 1,
            },
        ),
    ] {
        assert!(
            lower(&typed_model(ty, "y", expr), &book, &symbols, &configs()).is_err(),
            "programmatic {case} lowered"
        );
    }
    // And a well-typed programmatic model lowers.
    let lowered = lower(&model("y", Expr::LitInt(1)), &book, &symbols, &configs()).unwrap();
    assert_eq!(lowered.code.len(), 2);
    assert_eq!(lowered.code[0].command, "push_constant_string");
    assert_eq!(lowered.code[1].command, "pop_int_local");
}

#[test]
fn formatter_renders_assignments_canonically() {
    let text = format!("[]()\n{INTS}\n    $y=$a+41;\n    return(0);\n");
    let script = parse_source(&text, &symbols(), &configs(), &inames()).unwrap();
    let formatted = format_source(&script, &symbols(), &inames()).unwrap();
    assert!(
        formatted.contains("    $y = $a + 41;\n"),
        "got:\n{formatted}"
    );
}

/// One pass over the real 910 scripts pack for every expression family the
/// lifter produces: each script is dumped once (empty name registry, so no
/// call folding or value fusion, plus the pack's real config tables so the
/// config-typed reads resolve), the dumped text must assemble back to the
/// exact pack bytes, and each family must stay engaged corpus-wide. The
/// floors sit well below the counts measured on the frozen 910 cache
/// (14,313 scripts; see each message), so they only trip when a family
/// silently flattens. The
/// flat remainder is principled: results that feed `join_string`,
/// interface commands, returns, branches, or call arguments have no
/// `$target =` and stay flat by construction.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_dump_lifts_assignments() {
    use native910::pack::PackArchive;
    use native910::source::dump_script;

    let pack = common::require_pack(&["client.scripts.js5"]);
    let archive = PackArchive::open(&pack.join("client.scripts.js5")).expect("open scripts pack");
    let book = book();
    let symbols = symbols();
    let configs = ConfigTypes::load(&pack).expect("load config tables");
    // The nine operators added after the original `min`/`max` family.
    const NEW_OPS: &[&str] = &[
        "addpercent",
        "setbit",
        "clearbit",
        "testbit",
        "pow",
        "quickchat_dynamic_command_add",
        "append",
        "random",
        "randominc",
    ];
    /// Assignment lines carrying `needle`, and files with at least one.
    #[derive(Default)]
    struct Family {
        lines: usize,
        files: usize,
    }
    let families = [
        "scale(",
        "interpolate(",
        "tostring(",
        "join_string(",
        "struct_param(",
        "oc_param(",
        "_enum(",
    ];
    let mut counts: Vec<Family> = families.iter().map(|_| Family::default()).collect();
    let mut scripts = 0_usize;
    let mut assigned = Family::default();
    let mut new_op_lines = 0_usize;
    for group in archive.group_ids() {
        let files_in_group = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files_in_group {
            scripts += 1;
            let text = dump_script(bytes, &book, &symbols, &configs, &inames())
                .unwrap_or_else(|error| panic!("dump {group}/{file}: {error}"));
            let reassembled = assemble_source(&text, &book, &symbols, &configs, &inames())
                .unwrap_or_else(|error| panic!("assemble {group}/{file}: {error}\n{text}"));
            assert!(
                reassembled == *bytes,
                "{group}/{file}: dumped source does not reassemble to the pack bytes:\n{text}"
            );
            let mut hit = vec![false; families.len()];
            let mut hit_any = false;
            for line in text.lines() {
                let trimmed = line.trim_start();
                if !trimmed.starts_with('$') {
                    continue;
                }
                assigned.lines += 1;
                hit_any = true;
                if NEW_OPS.iter().any(|op| trimmed.contains(op)) {
                    new_op_lines += 1;
                }
                for (index, needle) in families.iter().enumerate() {
                    if trimmed.contains(needle) {
                        counts[index].lines += 1;
                        hit[index] = true;
                    }
                }
            }
            assigned.files += usize::from(hit_any);
            for (count, hit) in counts.iter_mut().zip(hit) {
                count.files += usize::from(hit);
            }
        }
    }
    eprintln!(
        "dump scan: {scripts} scripts, {} assignment lines over {} files ({new_op_lines} newer-operator lines)",
        assigned.lines, assigned.files
    );
    for (needle, count) in families.iter().zip(&counts) {
        eprintln!(
            "  {needle}: {} lines over {} files",
            count.lines, count.files
        );
    }
    let family = |needle: &str| &counts[families.iter().position(|n| *n == needle).unwrap()];
    // (count, floor, what): floors sit well below the measured counts.
    let floors: [(usize, usize, &str); 13] = [
        (assigned.lines, 44_000, "assignment lines (measured 54,250)"),
        (assigned.files, 4_300, "assignment files (measured 4,726)"),
        (new_op_lines, 3_000, "newer-operator lines (measured 3,911)"),
        (family("scale(").lines, 250, "scale lines (measured 347)"),
        (
            family("interpolate(").lines,
            2,
            "interpolate lines (both corpus uses)",
        ),
        (
            family("tostring(").lines,
            120,
            "tostring lines (measured 674)",
        ),
        (
            family("join_string(").lines,
            700,
            "join_string lines (measured 1,086)",
        ),
        (
            family("join_string(").files,
            220,
            "join_string files (measured 323)",
        ),
        (
            family("struct_param(").lines,
            3_000,
            "struct_param lines (measured 3,226)",
        ),
        (
            family("struct_param(").files,
            550,
            "struct_param files (measured 612)",
        ),
        (
            family("oc_param(").lines,
            450,
            "oc_param lines (measured 550)",
        ),
        (
            family("_enum(").lines,
            1_000,
            "_enum lines (measured 1,183)",
        ),
        (family("_enum(").files, 550, "_enum files (measured 614)"),
    ];
    for (got, floor, what) in floors {
        assert!(
            got >= floor,
            "{what}: lifting engaged far below the floor {floor}: {got}"
        );
    }
}

/// Real config ids for the equivalence pins below (all corpus-measured):
/// param 0 is int-kind (`kind 0`), param 65 is string-kind (`kind 36`);
/// enum 688 outputs int (`output 57`), enum 3907 outputs string
/// (`output 36`). Hand-built tables keep these hermetic (no pack needed).
fn real_configs() -> ConfigTypes {
    use native910::config::{EnumConfig, ParamConfig};
    use std::collections::BTreeMap;
    let params = BTreeMap::from([
        (
            0,
            ParamConfig {
                kind: Some(0),
                ..Default::default()
            },
        ),
        (
            65,
            ParamConfig {
                kind: Some(36),
                ..Default::default()
            },
        ),
        (
            132,
            ParamConfig {
                kind: Some(0),
                ..Default::default()
            },
        ),
        (
            3758,
            ParamConfig {
                kind: Some(0),
                ..Default::default()
            },
        ),
    ]);
    let enums = BTreeMap::from([
        (
            688,
            EnumConfig {
                output_type: Some(57),
                ..Default::default()
            },
        ),
        (
            3907,
            EnumConfig {
                output_type: Some(36),
                ..Default::default()
            },
        ),
        (
            425,
            EnumConfig {
                output_type: Some(9),
                ..Default::default()
            },
        ),
    ]);
    ConfigTypes::from_parts(params, enums)
}

fn assemble_with(decls: &str, body: &str, configs: &ConfigTypes) -> Vec<u8> {
    let text = format!("[]()\n{decls}\n{body}    return(0);\n");
    assemble_source(&text, &book(), &symbols(), configs, &inames())
        .unwrap_or_else(|error| panic!("assemble failed for:\n{text}\n{error}"))
}

fn assert_equiv_with(decls: &str, stmt: &str, flat: &str, configs: &ConfigTypes) {
    let expr_bytes = assemble_with(decls, &format!("    {stmt}\n"), configs);
    let flat_bytes = assemble_with(decls, flat, configs);
    assert_eq!(
        expr_bytes, flat_bytes,
        "expression `{stmt}` diverged from its flat equivalent"
    );
    let text = format!("[]()\n{decls}\n    {stmt}\n    return(0);\n");
    let first =
        parse_source(&text, &symbols(), configs, &inames()).expect("parse expression source");
    let formatted = format_source(&first, &symbols(), &inames()).expect("format expression source");
    let second =
        parse_source(&formatted, &symbols(), configs, &inames()).expect("reparse formatted source");
    assert_eq!(
        first.body, second.body,
        "parse∘format fixpoint broke for `{stmt}`"
    );
}

fn assert_rejects_with(decls: &str, stmt: &str, fragment: &str, configs: &ConfigTypes) {
    let text = format!("[]()\n{decls}\n    {stmt}\n    return(0);\n");
    let error = assemble_source(&text, &book(), &symbols(), configs, &inames())
        .expect_err(&format!("accepted but must reject: {stmt}"));
    assert!(
        error.to_string().contains(fragment),
        "error `{error}` misses `{fragment}` for `{stmt}`"
    );
}

#[test]
fn config_reads_match_flat_sequences() {
    let configs = real_configs();
    const INTS2: &str = "int $a, $b, $y;\n";
    const STR: &str = "string $s, $t;\nint $a, $y;\n";
    // Int-lane param reads (param 0, 132, 3758 are int-kind in the corpus).
    assert_equiv_with(
        INTS2,
        "$y = struct_param($a, 0);",
        "    push_int_local($a);\n    push_constant_string(0);\n    struct_param(0);\n    pop_int_local($y);\n",
        &configs,
    );
    assert_equiv_with(
        INTS2,
        "$y = struct_param(41, 132);",
        "    push_constant_string(41);\n    push_constant_string(132);\n    struct_param(0);\n    pop_int_local($y);\n",
        &configs,
    );
    assert_equiv_with(
        INTS2,
        "$y = oc_param($a, 3758);",
        "    push_int_local($a);\n    push_constant_string(3758);\n    oc_param(0);\n    pop_int_local($y);\n",
        &configs,
    );
    assert_equiv_with(
        INTS2,
        "$y = nc_param($a, 0);",
        "    push_int_local($a);\n    push_constant_string(0);\n    nc_param(0);\n    pop_int_local($y);\n",
        &configs,
    );
    // String-lane param reads (param 65 is string-kind).
    assert_equiv_with(
        STR,
        "$t = struct_param($a, 65);",
        "    push_int_local($a);\n    push_constant_string(65);\n    struct_param(0);\n    pop_string_local($t);\n",
        &configs,
    );
    assert_equiv_with(
        STR,
        "$t = oc_param($a, 65);",
        "    push_int_local($a);\n    push_constant_string(65);\n    oc_param(0);\n    pop_string_local($t);\n",
        &configs,
    );
    // Int-lane enum reads (688 outputs 57, 425 outputs 9 — both int-lane).
    assert_equiv_with(
        INTS2,
        "$y = _enum(0, 57, 688, $a);",
        "    push_constant_string(0);\n    push_constant_string(57);\n    push_constant_string(688);\n    push_int_local($a);\n    _enum(0);\n    pop_int_local($y);\n",
        &configs,
    );
    // String-lane enum reads (3907 outputs 36).
    assert_equiv_with(
        STR,
        "$t = _enum(0, 36, 3907, $a);",
        "    push_constant_string(0);\n    push_constant_string(36);\n    push_constant_string(3907);\n    push_int_local($a);\n    _enum(0);\n    pop_string_local($t);\n",
        &configs,
    );
    // Nesting: a config read inside int arithmetic, and int arithmetic as a
    // holder/key.
    assert_equiv_with(
        INTS2,
        "$y = struct_param($a + $b, 0);",
        "    push_int_local($a);\n    push_int_local($b);\n    add(0);\n    push_constant_string(0);\n    struct_param(0);\n    pop_int_local($y);\n",
        &configs,
    );
    assert_equiv_with(
        INTS2,
        "$y = _enum(0, 57, 688, $a + 1);",
        "    push_constant_string(0);\n    push_constant_string(57);\n    push_constant_string(688);\n    push_int_local($a);\n    push_constant_string(1);\n    add(0);\n    _enum(0);\n    pop_int_local($y);\n",
        &configs,
    );
}

#[test]
fn config_reads_reject_loudly() {
    let configs = real_configs();
    const INTS2: &str = "int $a, $y;\n";
    const STR: &str = "string $t;\nint $a, $y;\n";
    // Unknown ids (never guessed).
    assert_rejects_with(
        INTS2,
        "$y = struct_param($a, 999999);",
        "unknown param id 999999",
        &configs,
    );
    assert_rejects_with(
        INTS2,
        "$y = oc_param($a, 999999);",
        "unknown param id 999999",
        &configs,
    );
    assert_rejects_with(
        INTS2,
        "$y = _enum(0, 0, 999999, $a);",
        "unknown enum id 999999",
        &configs,
    );
    // Non-literal ids (the statically-known subset only).
    assert_rejects_with(
        INTS2,
        "$y = struct_param($a, $a);",
        "literal param id",
        &configs,
    );
    assert_rejects_with(
        INTS2,
        "$y = _enum(0, 0, $a, $a);",
        "literal enum id",
        &configs,
    );
    // Arity slips.
    assert_rejects_with(
        INTS2,
        "$y = struct_param($a, 1, 2);",
        "takes 2 arguments, got 3",
        &configs,
    );
    assert_rejects_with(
        INTS2,
        "$y = _enum(0, 57, 688);",
        "takes 4 arguments, got 3",
        &configs,
    );
    // Lane mismatches at assignment.
    assert_rejects_with(
        INTS2,
        "$y = struct_param($a, 65);",
        "cannot assign string expression to int",
        &configs,
    );
    assert_rejects_with(
        STR,
        "$t = struct_param($a, 0);",
        "cannot assign int expression to string",
        &configs,
    );
    assert_rejects_with(
        INTS2,
        "$y = _enum(0, 36, 3907, $a);",
        "cannot assign string expression to int",
        &configs,
    );
    assert_rejects_with(
        STR,
        "$t = _enum(0, 57, 688, $a);",
        "cannot assign int expression to string",
        &configs,
    );
    // Holder/key lanes are always int.
    assert_rejects_with(
        STR,
        "$t = struct_param($t, 65);",
        "holder operand",
        &configs,
    );
    // Missing config (empty resolver): every real id is unknown.
    assert_rejects_with(
        INTS2,
        "$y = struct_param($a, 0);",
        "unknown param id 0",
        &ConfigTypes::empty(),
    );
    assert_rejects_with(
        INTS2,
        "$y = _enum(0, 57, 688, $a);",
        "unknown enum id 688",
        &ConfigTypes::empty(),
    );
}
