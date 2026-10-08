//! Expression statements: human-authored computations over the flat 910 model.
//!
//! Today a computation is stack plumbing (`push`/`add`/`pop_int_local`); this
//! module lets humans author it as `$y = $a + 41 * $b;` instead. The binary
//! model stays flat: [`parse_expr`] builds trees, [`lower_expr`] emits the
//! identical push/operate/pop sequences, and [`format_expr`] prints them back.
//! Lifting (flat-to-tree recovery on dump) lives in
//! [`crate::source::lift`]: a `pop_*_local` terminating a straight-line run
//! of canonical pushes and the verified operators below becomes one
//! `$target = <expr>;` again, and a folded `~name(args)` call whose callee
//! has exactly one inferred return value fuses with its consuming pop into
//! `$target = ~name(args);`.
//!
//! Surface syntax (one `$name = <expr>;` statement per line):
//!
//! ```text
//! $y = $a + 41 * $b;
//! $y = min($a, max($b, 100));
//! $y = setbit($flags, 3);
//! $t = append($first, $last);
//! $s = join_string("a", $first, $last);
//! $n = string_length($s);
//! $ok = compare($s, "done");
//! $text = tostring($n);
//! $big = scale($a, $b, $c);
//! $v = struct_param($struct, 123);
//! $w = oc_param($obj, 65);
//! $e = _enum(0, 36, 425, $key);
//! $cid = bank/main;
//! ```
//!
//! Grammar:
//!
//! ```text
//! expr    := term ( "+" term )*
//! term    := primary ( ( "*" | "/" | "%" ) primary )*
//! primary := int | long | string | $local | call | component | "(" expr ")"
//!        := "-" number-literal (glued sign only: -5, -7L)
//! component := interface-name "/" ( child-name | child-number )
//!            (NAME-led only: `bank/7`, `bank/main` — a leading number stays
//!            division (`1253/4` is arithmetic, never a component), and the
//!            pair validates against the interface roster at parse, lowering
//!            to the identical packed int)
//! call    := "min" | "max" | "and" | "or" | "compare" | "addpercent"
//!            | "setbit" | "clearbit" | "testbit" | "pow"
//!            | "quickchat_dynamic_command_add" | "append" "(" expr "," expr ")"
//!        := "not" | "string_length" | "random" | "randominc" | "tostring" "(" expr ")"
//!        := "join_string" "(" expr "," expr ("," expr)* ")"  (2+ parts;
//!            the written count IS the arity — see [`join_shape`])
//!        := "scale" "(" expr "," expr "," expr ")"
//!        := "interpolate" "(" expr "," expr "," expr "," expr "," expr ")"
//!        := "struct_param" | "oc_param" | "nc_param" | "lc_param"
//!            | "seq_param" "(" expr "," int-literal ")"
//!            (second argument is a literal config id — the statically-known
//!            subset; any other shape fails loudly, see below)
//!        := "_enum" "(" expr "," expr "," int-literal "," expr ")"
//!            (third argument is a literal enum id — same subset rule)
//!        := "~" name "(" [expr ("," expr)*] ")"
//! int     := decimal i32 literal          (e.g. 41, -7, -2147483648)
//! long    := decimal i64 literal + "L"    (e.g. 41L, -7L)
//! string  := double-quoted, \\ \" \n \r \t escapes (mirrors the source
//!            string discipline; a // inside quotes is data, not a comment)
//! ```
//!
//! Precedence table (tightest first; `*` `/` `%` and `+` are left-associative):
//!
//! | Level | Operators | Shape        |
//! |-------|-----------|----------------------|
//! | 1     | primaries | literals, `$locals`, calls, `(expr)` |
//! | 2     | `*` `/` `%` | infix, left-associative |
//! | 3     | `+`       | infix, left-associative |
//!
//! [`format_expr`] parenthesizes only when needed (a child looser than its
//! parent, or a right child at the same level), so the output always reparses
//! to an equal model.
//!
//! Type rules (checked at parse with line errors, re-checked at lower with
//! message-only errors for programmatic models):
//!
//! | Operator | Operand types | Result |
//! |----------|---------------|--------|
//! | `+` `*` `/` `%` `min` `max` `and` `or` `addpercent` `setbit` `clearbit` `testbit` `pow` `quickchat_dynamic_command_add` | `(int, int)` | int |
//! | `not` `random` `randominc` | int | int |
//! | `tostring` | int | string |
//! | `Bank/7` component ref (`bank/main`, `bank/7`) | (validated name pair — packs to its int) | int |
//! | `scale` | `(int, int, int)` | int |
//! | `interpolate` | `(int, int, int, int, int)` | int |
//! | `compare` | `(string, string)` | int |
//! | `append` | `(string, string)` | string |
//! | `join_string(a, b, ...)` | 2+ strings (the written count is the arity) | string |
//! | `struct_param(obj, id)` `oc_param(obj, id)` `nc_param(obj, id)` `lc_param(obj, id)` `seq_param(obj, id)` | `(int, literal int config id)` | int or string per `ConfigTypes::param_is_string(id)` |
//! | `_enum(in, out, id, key)` | `(int, int, literal int enum id, int)` | int or string per `ConfigTypes::enum_output_is_string(id)` |
//! | `string_length` | string | int |
//! | `~name(args)` | callee signature, multiset-matched | the callee's single inferred return (exactly one value) |
//!
//! The (pop-types, push-type) behind each row is NOT restated here: the
//! fixed-lane shapes are read from the shared [`effect`](crate::effects::effect)
//! contract at every use (parse-time classification and lowering emission
//! both consult it), so drift between this module and the verified table is a
//! loud error, never a silent fork. The config-typed family below is the
//! principled exception: pops are fixed ints (verified against the same
//! handler bodies), but the push lane lives in config data, so typing,
//! lowering, and lifting resolve it through [`crate::config::ConfigTypes`]
//! (unknown or missing ids fail loudly, never guessed); the flat bytes stay
//! identical either way (`Byte(0)` operands throughout — corpus-measured).
//!
//! Operator set (every row proved against its retail handler; all ids below
//! are rows of the embedded 910 opcode book):
//!
//! | Syntax | Command (id) | Behaviour (pops -> pushes) |
//! |--------|----------------|--------------------------------|
//! | `a + b` | `add` (165) | pops two ints, pushes their sum (2 int -> 1 int) |
//! | `a * b` | `multiply` (586) | pops two ints, pushes their product (2 int -> 1 int) |
//! | `a / b` | `divide` (921) | pops two ints, pushes the quotient (2 int -> 1 int) |
//! | `a % b` | `modulo` (823) | pops two ints, pushes the remainder (2 int -> 1 int) |
//! | `min(a, b)` | `min` (590) | pops two ints, pushes the smaller (2 int -> 1 int) |
//! | `max(a, b)` | `max` (967) | pops two ints, pushes the larger (2 int -> 1 int) |
//! | `and(a, b)` | `and` (655) | pops two ints, pushes `a & b` (2 int -> 1 int) |
//! | `or(a, b)` | `or` (997) | pops two ints, pushes `a \| b` (2 int -> 1 int) |
//! | `addpercent(a, b)` | `addpercent` (235) | pops two ints, pushes `base * pct / 100 + base` (2 int -> 1 int) |
//! | `setbit(a, b)` | `setbit` (504) | pops two ints, pushes `a \| 1 << b` (2 int -> 1 int) |
//! | `clearbit(a, b)` | `clearbit` (274) | pops two ints, pushes `a & !(1 << b)` (2 int -> 1 int) |
//! | `testbit(a, b)` | `testbit` (1099) | pops two ints, pushes 1 when bit `b` of `a` is set, else 0 (2 int -> 1 int) |
//! | `pow(a, b)` | `pow` (1069) | pops two ints, pushes one int on both branches (`0` fast path, else the power) (2 int -> 1 int) |
//! | `quickchat_dynamic_command_add(a, b)` | `quickchat_dynamic_command_add` (869) | pops two ints, pushes their difference (2 int -> 1 int; subtraction despite the name) |
//! | `append(a, b)` | `append` (1072) | pops two strings, pushes their concatenation (2 obj -> 1 obj) |
//! | `join_string(a, b, ...)` | `join_string` (68) | pops `count` strings (the operand), pushes the in-order concatenation (`N` obj -> 1 obj, `N` = operand count; expression form takes `N >= 2`) |
//! | `not(x)` | `not` (766) | pops one int, pushes its bitwise complement (1 int -> 1 int) |
//! | `random(x)` | `random` (1375) | pops one int, pushes a random int below it (1 int -> 1 int) |
//! | `randominc(x)` | `randominc` (787) | pops one int, pushes a random int up to and including it (1 int -> 1 int) |
//! | `compare(a, b)` | `compare` (442) | pops two strings, pushes the comparison result (2 obj -> 1 int) |
//! | `string_length(x)` | `string_length` (253) | pops one string, pushes its length (1 obj -> 1 int) |
//! | `tostring(x)` | `tostring` (58) | pops one int, pushes its decimal text (1 int -> 1 obj) |
//! | `scale(a, b, c)` | `scale` (17) | pops three ints, pushes `a * c / b` (3 int -> 1 int) |
//! | `interpolate(a, b, c, d, e)` | `interpolate` (1142) | pops five ints, pushes the linear interpolation (5 int -> 1 int) |
//! | `struct_param(obj, id)` | `struct_param` (39) | pops two ints, then takes the param's string-type branch (2 int -> 1 int-or-obj per config) |
//! | `oc_param(obj, id)` | `oc_param` (913) | pops two ints, same branch over the obj config (2 int -> 1 int-or-obj per config) |
//! | `nc_param(obj, id)` | `nc_param` (87) | pops two ints, same branch over the npc config (2 int -> 1 int-or-obj per config) |
//! | `lc_param(obj, id)` | `lc_param` (1162) | pops two ints, same branch over the loc config (2 int -> 1 int-or-obj per config) |
//! | `seq_param(obj, id)` | `seq_param` (1390) | pops two ints, same branch over the seq config (2 int -> 1 int-or-obj per config) |
//! | `_enum(in, out, id, key)` | `_enum` (810) | pops four ints with a loud input/output-type guard, then an output type of string selects the lane (4 int -> 1 int-or-obj per the enum's output type) |
//!
//! Exclusion list (each rejected loudly, with reasons):
//!
//! * Numeric-led `1253/4`: division, never a component — the `/` with two
//!   numbers is already taken by `parse_expr`, and component syntax is
//!   NAME-led only (`bank/7`). Unnamed interfaces keep the numeric packed-int
//!   spelling (status quo), which makes curation the incentive.
//! * Binary or general unary `-`: no `subtract`/`negate` handler exists in
//!   the retail command dispatch and no such row exists in the opcode book
//!   (only unrelated `if_setsubtractinsets` UI commands). A `-` glued to
//!   digits is a negative literal, not an operator. (`quickchat_dynamic_
//!   command_add` IS a subtract body but it surfaces under
//!   its own canonical command word above, never as `-`.)
//! * `~name(...)` calls in value position: resolved against the registry;
//!   unknown names, arity mismatches, and callees without exactly one
//!   statically inferred return value (unknown, void, or multi-value) all
//!   fail loudly.
//! * Config-typed reads (`struct_param` / `oc_param` / `nc_param` /
//!   `lc_param` / `seq_param` / `_enum`): INCLUDED with the statically-known
//!   restriction — the config id (second argument for the 2-pop param
//!   family, third for the 4-pop `_enum`) must be an integer literal so the
//!   lane resolves through [`crate::config::ConfigTypes`]. Non-literal ids,
//!   unknown ids, and enums with no output type fail loudly; lowering emits
//!   the identical `Byte(0)` command bytes either way. (`cc_param` stays
//!   excluded: same branch but its operand varies `Byte(0/1)` for the
//!   secondary-context flag, so it does not share this design's fixed-operand
//!   shape. `db_getfield` stays excluded: 3 int pops pushing a variable
//!   column tuple, never one value. `enum_getoutputcount` stays excluded:
//!   a fixed 1-int -> 1-int row with no lane branch, a plain unary when its
//!   own workstream claims it.)
//! * The `append_num` / `append_signnum` siblings of `append`: MIXED-lane
//!   pops, proved separately against their handler bodies — `append_num`
//!   pops the string, then the int, pushing one string (`[1,1,0]` pops -> `[0,1,0]`
//!   push); `append_signnum` is the same shape with a `+n` sign prefix on
//!   non-negatives. Neither the homogeneous binary rows nor the homogeneous
//!   n-ary `join_string` design covers a heterogeneous pair, so both stay
//!   flat until a mixed-lane binary shape lands. Measured 5 corpus uses
//!   (`append_num` in 2 files; `append_signnum` unused) — flat is exact
//!   there. Bare `append` (2 obj -> 1 obj) is verified and included.
//! * Higher-arity fixed producers beyond the n-ary set: only `scale` (3 int ->
//!   1 int) and `interpolate` (5 int -> 1 int) are admitted as
//!   [`Expr::Nary`] nodes, each proved against its handler body above. Any
//!   further fixed arity needs its own `NaryOp` row first — arities never
//!   fold into binary trees silently. (The operand-counted `join_string` is
//!   variadic, not fixed-arity: it rides its own [`Expr::Join`] node with
//!   the count as its explicit operand — see above.)
//! * Other config- or state-dependent int producers: excluded until their
//!   shapes prove fixed (each must survive step 1 against its handler body).
//! * Long arithmetic: the ALU handlers above are int-stack-only; longs pass
//!   through as literals and `$locals` only.
//! * Bare computed values with no `$target =`: every expression statement
//!   must assign (unpopped stack values are garbage) — rejected.
//! * Undeclared targets or `$locals`, and type mismatches: rejected.

use crate::config::ConfigTypes;
use crate::effects::{Effect, effect};
use crate::error::{NativeError, Result};
use crate::inames::InterfaceRegistry;
use crate::opcode::OpcodeBook;
use crate::script::{Counts, Instruction, Operand};
use crate::source::ValType;
use crate::symbols::SymbolRegistry;
use std::collections::BTreeMap;
use std::fmt;

/// Cap on `(`/call nesting inside one expression: hostile one-line input
/// must error, never overflow the parser stack. 64 is far beyond any
/// human-authored computation.
const MAX_NESTING: usize = 64;

/// Minimum accepted `join_string` arity: binary and up. Counts 0/1 are
/// handler-valid (the empty string, the identity copy) but have no expression
/// form — a degenerate join hides an authoring error, and the corpus never
/// uses them (measured 2..=33 over 4,060 uses) — so the parser, the typers,
/// and the dump-side folder all reject or skip them. The flat
/// `join_string(0/1)` spelling stays available and assembles untouched.
pub(crate) const MIN_JOIN_ARITY: usize = 2;

/// An expression: literals, `$locals`, and the verified operator set above.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    /// Int literal (`41`, `-7`).
    LitInt(i32),
    /// Long literal (`41L`).
    LitLong(i64),
    /// Packed component reference (`bank/7`, `bank/main`): an int-literal
    /// spelling for `iface << 16 | child`, validated against the interface
    /// roster at parse (see [`InterfaceRegistry::resolve_component`]) and
    /// lowering to the identical `push_constant_string` int. A bare number is
    /// [`Expr::LitInt`]; only the NAME-led `Name/child` shape becomes this.
    Component {
        /// Interface (pack group) id.
        iface: i32,
        /// Child (pack file) index.
        child: u32,
    },
    /// String literal (`"text"`).
    LitStr(String),
    /// Named local or argument slot (`$total`; `$`-less name inside).
    Local(String),
    /// One verified unary operator applied to an operand.
    Unary(UnaryOp, Box<Self>),
    /// One verified binary operator applied to left then right.
    Binary(BinaryOp, Box<Self>, Box<Self>),
    /// One verified variadic string join applied to its parts in listed
    /// order (left-first on the stack). Unlike [`NaryOp`] rows — fixed
    /// arities with a zero generic operand — the part count IS the explicit
    /// `join_string` count operand: parse, typing, lowering, and lifting all
    /// tie the tree length to it (see [`join_shape`]), and any mismatch fails
    /// loudly. Every part is a string; the result is a string.
    Join(Vec<Self>),
    /// One verified fixed-arity (3+) operator applied to its operands in
    /// listed order (left-first on the stack). A single n-ary category covers
    /// every admitted higher arity — `scale` today, `interpolate` today —
    /// with the exact arity pinned per operator (see [`NaryOp::arity`]) and
    /// the shape read from the shared effect contract (see [`nary_shape`]),
    /// never restated.
    Nary(NaryOp, Vec<Self>),
    /// One verified config-backed param read (`struct_param`, `oc_param`,
    /// `nc_param`, `lc_param`, `seq_param`): `obj` is any int expression
    /// (the holder id); `param` is a literal config id, never an arbitrary
    /// expression — the statically-known subset the lane resolver needs.
    /// The result is int or string per
    /// [`ConfigTypes::param_is_string`](crate::config::ConfigTypes::param_is_string).
    /// Lowering emits the holder push, the literal id push, then the command
    /// with its canonical `Byte(0)` operand (corpus-measured: all five words
    /// carry `Byte(0)` at every one of their 10k+ uses).
    Param {
        /// Which holder table the read addresses.
        op: ParamOp,
        /// Holder id expression (any int).
        obj: Box<Self>,
        /// Literal param id (the config key).
        param: i32,
    },
    /// One verified config-backed enum read (`_enum`): `input`, `output`,
    /// and `key` are any int expressions; `enumeration` is a literal enum id.
    /// The result is int or string per
    /// [`ConfigTypes::enum_output_is_string`](crate::config::ConfigTypes::enum_output_is_string).
    /// Lowering emits the three pushes in listed order, the literal enum-id
    /// push, the key push, then `_enum` with `Byte(0)` (corpus-measured:
    /// `Byte(0)` at all 4,086 uses).
    Enum {
        /// Declared input-type id expression (any int; mismatch with the
        /// enum's config input throws at runtime, preserved byte-exact).
        input: Box<Self>,
        /// Declared output-type id expression (any int; same throwing guard).
        output: Box<Self>,
        /// Literal enum id (the config key).
        enumeration: i32,
        /// Lookup key expression (any int).
        key: Box<Self>,
    },
    /// A call used as a value (`~bank(41, $x)`): the callee must be
    /// registry-known with exactly one statically inferred return value.
    /// Resolution, arity, and return checks happen at typing and lowering
    /// (which own the registry), never at parse: the parser records the name
    /// and the raw argument trees.
    Call {
        /// `$`-less callee name (no tilde inside).
        name: String,
        /// Argument expressions, in listed order.
        args: Vec<Self>,
    },
}

/// The verified unary operators: `not`, `random`, `randominc` (int -> int),
/// `string_length` (string -> int), and `tostring` (int -> string). Shapes
/// come from the shared effect contract, not from this enum — see
/// [`unary_shape`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    /// `not(x)` — bitwise complement.
    Not,
    /// `random(x)` — a random int in `0..x`.
    Random,
    /// `randominc(x)` — a random int in `0..=x`.
    RandomInc,
    /// `string_length(x)` — string length.
    StringLength,
    /// `tostring(x)` — decimal rendering.
    ToString,
}

/// The verified binary operators: four infix int ops plus twelve
/// named-function-form ops. Shapes come from the shared effect contract —
/// see [`binary_shape`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    /// `a + b`.
    Add,
    /// `a * b`.
    Multiply,
    /// `a / b`.
    Divide,
    /// `a % b`.
    Modulo,
    /// `min(a, b)`.
    Min,
    /// `max(a, b)`.
    Max,
    /// `and(a, b)`.
    And,
    /// `or(a, b)`.
    Or,
    /// `addpercent(a, b)` — `a + a * b / 100`.
    AddPercent,
    /// `setbit(a, b)` — `a | 1 << b`.
    SetBit,
    /// `clearbit(a, b)` — `a & ~(1 << b)`.
    ClearBit,
    /// `testbit(a, b)` — `(a & 1 << b) != 0`.
    TestBit,
    /// `pow(a, b)` — `a ^ b` (`0 ^ _` is `0`).
    Pow,
    /// `quickchat_dynamic_command_add(a, b)` — `a - b` (subtraction despite
    /// the name).
    QuickchatDynamicCommandAdd,
    /// `append(a, b)` — string concatenation.
    Append,
    /// `compare(a, b)`.
    Compare,
}

impl UnaryOp {
    /// The verified 910 command this operator lowers to.
    #[must_use]
    pub fn command(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::Random => "random",
            Self::RandomInc => "randominc",
            Self::StringLength => "string_length",
            Self::ToString => "tostring",
        }
    }
}

impl BinaryOp {
    /// The verified 910 command this operator lowers to.
    #[must_use]
    pub fn command(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Multiply => "multiply",
            Self::Divide => "divide",
            Self::Modulo => "modulo",
            Self::Min => "min",
            Self::Max => "max",
            Self::And => "and",
            Self::Or => "or",
            Self::AddPercent => "addpercent",
            Self::SetBit => "setbit",
            Self::ClearBit => "clearbit",
            Self::TestBit => "testbit",
            Self::Pow => "pow",
            Self::QuickchatDynamicCommandAdd => "quickchat_dynamic_command_add",
            Self::Append => "append",
            Self::Compare => "compare",
        }
    }
}

/// The verified fixed-arity (3+) operators: `scale` and `interpolate` (both
/// all-int producers). Shapes come from the shared effect contract, not from
/// this enum — see [`nary_shape`]; the arity below pins how many operands an
/// [`Expr::Nary`] node must carry, and any contract drift (a pop count that
/// disagrees with it) is a loud error at typing and lowering alike.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NaryOp {
    /// `scale(a, b, c)` — `a * c / b`.
    Scale,
    /// `interpolate(a, b, c, d, e)` — `(b - a) * (e - c) / (d - c) + a`.
    Interpolate,
}

impl NaryOp {
    /// The verified 910 command this operator lowers to.
    #[must_use]
    pub fn command(self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Interpolate => "interpolate",
        }
    }

    /// How many operands an [`Expr::Nary`] node with this operator carries.
    #[must_use]
    pub fn arity(self) -> usize {
        match self {
            Self::Scale => 3,
            Self::Interpolate => 5,
        }
    }
}

/// Which holder table a config-backed [`Expr::Param`] read addresses. All
/// five share the exact handler shape (two int pops, then the param's
/// string-type branch) over different holder lists — one design,
/// five words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamOp {
    /// `struct_param(obj, id)` over `StructConfig`.
    StructParam,
    /// `oc_param(obj, id)` over the obj config.
    OcParam,
    /// `nc_param(obj, id)` over the npc config.
    NcParam,
    /// `lc_param(obj, id)` over the loc config.
    LcParam,
    /// `seq_param(obj, id)` over the seq config.
    SeqParam,
}

impl ParamOp {
    /// The verified 910 command this operator lowers to.
    #[must_use]
    pub fn command(self) -> &'static str {
        match self {
            Self::StructParam => "struct_param",
            Self::OcParam => "oc_param",
            Self::NcParam => "nc_param",
            Self::LcParam => "lc_param",
            Self::SeqParam => "seq_param",
        }
    }

    /// Parse a command word into its holder, if it names one.
    #[must_use]
    pub fn from_command(command: &str) -> Option<Self> {
        match command {
            "struct_param" => Some(Self::StructParam),
            "oc_param" => Some(Self::OcParam),
            "nc_param" => Some(Self::NcParam),
            "lc_param" => Some(Self::LcParam),
            "seq_param" => Some(Self::SeqParam),
            _ => None,
        }
    }
}

/// A unary operator's (operand type, result type), read from the shared
/// effect contract: `not`, `random`, and `randominc` are the 1-int -> 1-int
/// row, `string_length` the 1-obj -> 1-int row, `tostring` the 1-int -> 1-obj
/// row. Anything else (or a contract drift) is an error.
fn unary_shape(command: &str) -> std::result::Result<(ValType, ValType), String> {
    match effect(command, &Operand::Byte(0)) {
        Effect::Fixed {
            pops: [1, 0, 0],
            pushes: [1, 0, 0],
        } => Ok((ValType::Int, ValType::Int)),
        Effect::Fixed {
            pops: [0, 1, 0],
            pushes: [1, 0, 0],
        } => Ok((ValType::String, ValType::Int)),
        Effect::Fixed {
            pops: [1, 0, 0],
            pushes: [0, 1, 0],
        } => Ok((ValType::Int, ValType::String)),
        _ => Err(format!(
            "operator '{command}' is not a unary expression operator"
        )),
    }
}

/// A binary operator's ([left, right] types, result type), read from the
/// shared effect contract: the int ALU rows are (int, int) -> int,
/// `compare` is (string, string) -> int, `append` is (string, string) ->
/// string. Anything else is an error.
fn binary_shape(command: &str) -> std::result::Result<([ValType; 2], ValType), String> {
    match effect(command, &Operand::Byte(0)) {
        Effect::Fixed {
            pops: [2, 0, 0],
            pushes: [1, 0, 0],
        } => Ok(([ValType::Int, ValType::Int], ValType::Int)),
        Effect::Fixed {
            pops: [0, 2, 0],
            pushes: [1, 0, 0],
        } => Ok(([ValType::String, ValType::String], ValType::Int)),
        Effect::Fixed {
            pops: [0, 2, 0],
            pushes: [0, 1, 0],
        } => Ok(([ValType::String, ValType::String], ValType::String)),
        _ => Err(format!(
            "operator '{command}' is not a binary expression operator"
        )),
    }
}

/// An n-ary operator's ((operand lane, operand count), result type), read
/// from the shared effect contract: every admitted row is homogeneous
/// (exactly one pop lane) pushing exactly one value — `scale` the 3-int ->
/// 1-int row, `interpolate` the 5-int -> 1-int row. The contract supplies the
/// count; the caller additionally pins it against [`NaryOp::arity`], so drift
/// either way is an error.
fn nary_shape(command: &str) -> std::result::Result<((ValType, usize), ValType), String> {
    match effect(command, &Operand::Byte(0)) {
        Effect::Fixed { pops, pushes } => {
            let (lane, count) = crate::source::single_lane(pops).ok_or_else(|| {
                format!("operator '{command}' is not an n-ary expression operator")
            })?;
            if count < 3 {
                return Err(format!(
                    "operator '{command}' is not an n-ary expression operator"
                ));
            }
            let (result, pushed) = crate::source::single_lane(pushes).ok_or_else(|| {
                format!("operator '{command}' is not an n-ary expression operator")
            })?;
            if pushed != 1 {
                return Err(format!(
                    "operator '{command}' is not an n-ary expression operator"
                ));
            }
            Ok(((lane, count), result))
        }
        _ => Err(format!(
            "operator '{command}' is not an n-ary expression operator"
        )),
    }
}

/// A `join_string` part count tied to its tree arity: `len` (the tree's own
/// length — the count is written in the source text itself, so there is only
/// one value to check, never two to drift) must clear [`MIN_JOIN_ARITY`],
/// fit the shared triple's `u16` lanes, and reproduce the operand-aware
/// contract row `effect("join_string", Count(len)) == Pure { [0, len, 0],
/// [0, 1, 0] }`. Degenerate counts, triple-overflowing counts, and contract
/// drift all fail with message-only errors the caller wraps.
fn join_shape(len: usize) -> std::result::Result<u16, String> {
    if len < MIN_JOIN_ARITY {
        return Err(format!(
            "operator 'join_string' takes at least {MIN_JOIN_ARITY} arguments, got {len}"
        ));
    }
    let narrow = u16::try_from(len).map_err(|_| {
        format!(
            "operator 'join_string' takes at most {} arguments, got {len}",
            u16::MAX
        )
    })?;
    match effect("join_string", &Operand::Count(i32::from(narrow))) {
        Effect::Fixed { pops, pushes } if pops == [0, narrow, 0] && pushes == [0, 1, 0] => {
            Ok(narrow)
        }
        _ => Err(
            "effect drift for 'join_string': shared contract disagrees with the expression table"
                .to_string(),
        ),
    }
}

/// The static type of an expression. `local_ty` resolves `$`-less names
/// (the caller owns declarations: the parser reads them off its tables, the
/// lowerer off its slot map); `symbols` resolves calls; `configs` resolves
/// the config-typed family (`struct_param` and siblings, `_enum`) — unknown
/// or missing config ids fail loudly, never guessed. Undeclared `$locals`,
/// mistyped operator arguments, non-literal config ids, unknown callees,
/// arity mismatches, and unusable return arities fail with message-only
/// errors the caller wraps.
pub fn expr_type(
    expr: &Expr,
    local_ty: &dyn Fn(&str) -> Option<ValType>,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
) -> std::result::Result<ValType, String> {
    match expr {
        Expr::LitInt(_) => Ok(ValType::Int),
        Expr::LitLong(_) => Ok(ValType::Long),
        // A validated component ref IS its packed int: no registry needed
        // here (resolution happened at parse), and programmatic pairs type
        // exactly like the literal they spell.
        Expr::Component { .. } => Ok(ValType::Int),
        Expr::LitStr(_) => Ok(ValType::String),
        Expr::Local(name) => local_ty(name).ok_or_else(|| format!("undeclared local '${name}'")),
        Expr::Call { name, args } => check_call(name, args, symbols, &|arg| {
            expr_type(arg, local_ty, symbols, configs)
        })
        .map(|(_, result)| result),
        Expr::Unary(op, inner) => {
            let (expect, result) = unary_shape(op.command())?;
            let got = expr_type(inner, local_ty, symbols, configs)?;
            if got != expect {
                return Err(format!(
                    "operator '{}' needs a {} operand, got {}",
                    op.command(),
                    expect.keyword(),
                    got.keyword()
                ));
            }
            Ok(result)
        }
        Expr::Binary(op, left, right) => {
            let ([expect_left, expect_right], result) = binary_shape(op.command())?;
            let got_left = expr_type(left, local_ty, symbols, configs)?;
            let got_right = expr_type(right, local_ty, symbols, configs)?;
            if got_left != expect_left || got_right != expect_right {
                return Err(format!(
                    "operator '{}' needs ({}, {}) operands, got ({}, {})",
                    op.command(),
                    expect_left.keyword(),
                    expect_right.keyword(),
                    got_left.keyword(),
                    got_right.keyword()
                ));
            }
            Ok(result)
        }
        Expr::Nary(op, args) => {
            let command = op.command();
            let ((expect, count), result) = nary_shape(command)?;
            if args.len() != op.arity() || args.len() != count {
                return Err(format!(
                    "operator '{command}' takes {} arguments, got {}",
                    op.arity(),
                    args.len()
                ));
            }
            let mut gots = Vec::with_capacity(args.len());
            for arg in args {
                gots.push(expr_type(arg, local_ty, symbols, configs)?);
            }
            if gots.iter().any(|got| *got != expect) {
                let want = vec![expect.keyword(); args.len()].join(", ");
                let got = gots
                    .iter()
                    .map(|got| got.keyword())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "operator '{command}' needs ({want}) operands, got ({got})"
                ));
            }
            Ok(result)
        }
        Expr::Param { op, obj, param } => {
            let command = op.command();
            let got = expr_type(obj, local_ty, symbols, configs)?;
            if got != ValType::Int {
                return Err(format!(
                    "operator '{command}' needs a {} holder operand, got {}",
                    ValType::Int.keyword(),
                    got.keyword()
                ));
            }
            let is_string = configs
                .param_is_string(*param)
                .map_err(|error| error.to_string())?;
            Ok(if is_string {
                ValType::String
            } else {
                ValType::Int
            })
        }
        Expr::Enum {
            input,
            output,
            enumeration,
            key,
        } => {
            for (what, arg) in [("input", input), ("output", output), ("key", key)] {
                let got = expr_type(arg, local_ty, symbols, configs)?;
                if got != ValType::Int {
                    return Err(format!(
                        "operator '_enum' needs int {what} operands, got {}",
                        got.keyword()
                    ));
                }
            }
            let is_string = configs
                .enum_output_is_string(*enumeration)
                .map_err(|error| error.to_string())?;
            Ok(if is_string {
                ValType::String
            } else {
                ValType::Int
            })
        }
        Expr::Join(parts) => {
            // The tree length IS the count: `join_shape` ties it to the
            // explicit-count contract row (arity-vs-count mismatches and
            // degenerate counts fail here), then every part must be a string.
            join_shape(parts.len())?;
            let mut gots = Vec::with_capacity(parts.len());
            for part in parts {
                gots.push(expr_type(part, local_ty, symbols, configs)?);
            }
            if gots.iter().any(|got| *got != ValType::String) {
                let got = gots
                    .iter()
                    .map(|got| got.keyword())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "operator 'join_string' needs string operands, got ({got})"
                ));
            }
            Ok(ValType::String)
        }
    }
}

/// Pack a resolved `(iface, child)` pair back to its wire int
/// (`iface << 16 | child`). Fails on out-of-range parts rather than wrapping;
/// validated pairs always pack, so only programmatic garbage reaches the error.
/// Shared with [`crate::source`] so statement operands pack exactly like
/// expression leaves. (`checked_mul` — not `checked_shl`, which only guards
/// the shift amount and silently drops high bits.)
pub(crate) fn pack_component_ref(iface: i32, child: u32) -> Option<i32> {
    let narrow = i32::try_from(child).ok()?;
    iface.checked_mul(65536)?.checked_add(narrow)
}

/// The push triple for one produced value of type `ty`: exactly one push on
/// its own lane. Every expression operator pushes exactly one value (the
/// dump-side builder rejects anything else), so this is the only push shape
/// lowering ever emits for operators.
fn pushes_for(ty: ValType) -> [u16; 3] {
    match ty {
        ValType::Int => [1, 0, 0],
        ValType::String => [0, 1, 0],
        ValType::Long => [0, 0, 1],
    }
}

/// Lower an expression to canonical instructions appended to `out`,
/// returning the produced value's type.
///
/// Literals lower to the corpus-measured `push_constant_string` family
/// spellings (exactly like call arguments in `desugar_calls`: int-tagged
/// ints, long-tagged longs, strings); `$locals` lower through their declared
/// types to the typed `push_*_local`. Operators lower left-operand first,
/// then right, then their verified command with a correctly-filled opcode id
/// from `book` (unknown command = error, never a placeholder);
/// `join_string` lowers each part in order, then the command with its
/// explicit count operand (never a zero generic — the flat form spells its
/// count, and so does this). Config-typed reads lower their holder/type-id
/// pushes in listed order (the literal config id re-emits as a
/// `push_constant_string` int, exactly the corpus spelling), then the command
/// with its canonical `Byte(0)` operand via [`emit_config`] (the shared
/// effect contract is `Unknown` for these by design — no config access — so
/// the opcode still comes from the book and the operand is the measured
/// zero, never a placeholder). Calls lower
/// each argument in order, then the `gosub` itself. Every fixed-lane emission
/// is cross-checked against the shared effect contract: a stated (pops,
/// pushes) triple that disagrees with `effect()` fails loudly instead of
/// emitting bytes no validator agrees with.
pub fn lower_expr(
    expr: &Expr,
    slots: &BTreeMap<&str, (ValType, i32)>,
    book: &OpcodeBook,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
    out: &mut Vec<Instruction>,
) -> Result<ValType> {
    match expr {
        Expr::LitInt(value) => {
            emit(
                book,
                out,
                "push_constant_string",
                Operand::Int(*value),
                [0, 0, 0],
                [1, 0, 0],
            )?;
            Ok(ValType::Int)
        }
        Expr::LitLong(value) => {
            emit(
                book,
                out,
                "push_constant_string",
                Operand::Long(*value),
                [0, 0, 0],
                [0, 0, 1],
            )?;
            Ok(ValType::Long)
        }
        Expr::Component { iface, child } => {
            // The identical int the numeric spelling emits (see the
            // `LitInt` arm): symbolic and numeric lower byte-for-byte equal.
            let packed = pack_component_ref(*iface, *child).ok_or_else(|| {
                NativeError::Invalid(format!(
                    "component {iface}/{child} does not pack into an int"
                ))
            })?;
            emit(
                book,
                out,
                "push_constant_string",
                Operand::Int(packed),
                [0, 0, 0],
                [1, 0, 0],
            )?;
            Ok(ValType::Int)
        }
        Expr::LitStr(text) => {
            emit(
                book,
                out,
                "push_constant_string",
                Operand::Str(text.clone()),
                [0, 0, 0],
                [0, 1, 0],
            )?;
            Ok(ValType::String)
        }
        Expr::Local(name) => {
            let (ty, slot) = slots
                .get(name.as_str())
                .copied()
                .ok_or_else(|| NativeError::Invalid(format!("undeclared local '${name}'")))?;
            let command = match ty {
                ValType::Int => "push_int_local",
                ValType::String => "push_string_local",
                ValType::Long => "push_long_local",
            };
            let pushes = match ty {
                ValType::Int => [1, 0, 0],
                ValType::String => [0, 1, 0],
                ValType::Long => [0, 0, 1],
            };
            emit(book, out, command, Operand::Local(slot), [0, 0, 0], pushes)?;
            Ok(ty)
        }
        Expr::Unary(op, inner) => {
            let got = lower_expr(inner, slots, book, symbols, configs, out)?;
            let command = op.command();
            let (expect, result) = unary_shape(command).map_err(NativeError::Invalid)?;
            if got != expect {
                return Err(NativeError::Invalid(format!(
                    "operator '{command}' needs a {} operand, got {}",
                    expect.keyword(),
                    got.keyword()
                )));
            }
            let pops = match expect {
                ValType::Int => [1, 0, 0],
                ValType::String => [0, 1, 0],
                ValType::Long => [0, 0, 1],
            };
            emit(
                book,
                out,
                command,
                Operand::Byte(0),
                pops,
                pushes_for(result),
            )?;
            Ok(result)
        }
        Expr::Binary(op, left, right) => {
            let got_left = lower_expr(left, slots, book, symbols, configs, out)?;
            let got_right = lower_expr(right, slots, book, symbols, configs, out)?;
            let command = op.command();
            let ([expect_left, expect_right], result) =
                binary_shape(command).map_err(NativeError::Invalid)?;
            if got_left != expect_left || got_right != expect_right {
                return Err(NativeError::Invalid(format!(
                    "operator '{command}' needs ({}, {}) operands, got ({}, {})",
                    expect_left.keyword(),
                    expect_right.keyword(),
                    got_left.keyword(),
                    got_right.keyword()
                )));
            }
            let pops = match expect_left {
                ValType::Int => [2, 0, 0],
                ValType::String => [0, 2, 0],
                ValType::Long => [0, 0, 2],
            };
            emit(
                book,
                out,
                command,
                Operand::Byte(0),
                pops,
                pushes_for(result),
            )?;
            Ok(result)
        }
        Expr::Nary(op, args) => {
            let command = op.command();
            let ((expect, count), result) = nary_shape(command).map_err(NativeError::Invalid)?;
            // Arity before emission: a programmatic model with the wrong
            // operand count fails without emitting a partial sequence.
            if args.len() != op.arity() || args.len() != count {
                return Err(NativeError::Invalid(format!(
                    "operator '{command}' takes {} arguments, got {}",
                    op.arity(),
                    args.len()
                )));
            }
            let mut gots = Vec::with_capacity(args.len());
            for arg in args {
                gots.push(lower_expr(arg, slots, book, symbols, configs, out)?);
            }
            if gots.iter().any(|got| *got != expect) {
                let want = vec![expect.keyword(); args.len()].join(", ");
                let got = gots
                    .iter()
                    .map(|got| got.keyword())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(NativeError::Invalid(format!(
                    "operator '{command}' needs ({want}) operands, got ({got})"
                )));
            }
            let wide = u16::try_from(count).map_err(|_| {
                NativeError::Invalid(format!(
                    "operator '{command}' arity {count} does not fit the stack triple"
                ))
            })?;
            let pops = match expect {
                ValType::Int => [wide, 0, 0],
                ValType::String => [0, wide, 0],
                ValType::Long => [0, 0, wide],
            };
            emit(
                book,
                out,
                command,
                Operand::Byte(0),
                pops,
                pushes_for(result),
            )?;
            Ok(result)
        }
        Expr::Join(parts) => {
            // Arity before emission, like the fixed n-ary arm above: a
            // programmatic model with a degenerate or triple-overflowing
            // part count fails without emitting a partial sequence. The
            // count is the tree length itself — there is no second value to
            // mismatch — and `emit` cross-checks it against the
            // operand-aware contract row.
            let narrow = join_shape(parts.len()).map_err(NativeError::Invalid)?;
            let mut gots = Vec::with_capacity(parts.len());
            for part in parts {
                gots.push(lower_expr(part, slots, book, symbols, configs, out)?);
            }
            if gots.iter().any(|got| *got != ValType::String) {
                let got = gots
                    .iter()
                    .map(|got| got.keyword())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(NativeError::Invalid(format!(
                    "operator 'join_string' needs string operands, got ({got})"
                )));
            }
            emit(
                book,
                out,
                "join_string",
                Operand::Count(i32::from(narrow)),
                [0, narrow, 0],
                [0, 1, 0],
            )?;
            Ok(ValType::String)
        }
        Expr::Param { op, obj, param } => {
            // Validate first without emitting (holder must be int, lane from
            // config — shared with `expr_type` so both agree), then emit the
            // holder, the literal id push, and the command itself.
            let command = op.command();
            let got = lower_expr(obj, slots, book, symbols, configs, out)?;
            if got != ValType::Int {
                return Err(NativeError::Invalid(format!(
                    "operator '{command}' needs a {} holder operand, got {}",
                    ValType::Int.keyword(),
                    got.keyword()
                )));
            }
            let is_string = configs.param_is_string(*param)?;
            let result = if is_string {
                ValType::String
            } else {
                ValType::Int
            };
            emit(
                book,
                out,
                "push_constant_string",
                Operand::Int(*param),
                [0, 0, 0],
                [1, 0, 0],
            )?;
            emit_config(book, out, command, [2, 0, 0], pushes_for(result))?;
            Ok(result)
        }
        Expr::Enum {
            input,
            output,
            enumeration,
            key,
        } => {
            let got_input = lower_expr(input, slots, book, symbols, configs, out)?;
            let got_output = lower_expr(output, slots, book, symbols, configs, out)?;
            if got_input != ValType::Int || got_output != ValType::Int {
                return Err(NativeError::Invalid(format!(
                    "operator '_enum' needs int input/output operands, got ({}, {})",
                    got_input.keyword(),
                    got_output.keyword()
                )));
            }
            emit(
                book,
                out,
                "push_constant_string",
                Operand::Int(*enumeration),
                [0, 0, 0],
                [1, 0, 0],
            )?;
            let got_key = lower_expr(key, slots, book, symbols, configs, out)?;
            if got_key != ValType::Int {
                return Err(NativeError::Invalid(format!(
                    "operator '_enum' needs an int key operand, got {}",
                    got_key.keyword()
                )));
            }
            let is_string = configs.enum_output_is_string(*enumeration)?;
            let result = if is_string {
                ValType::String
            } else {
                ValType::Int
            };
            emit_config(book, out, "_enum", [4, 0, 0], pushes_for(result))?;
            Ok(result)
        }
        Expr::Call { name, args } => {
            // Validate first without emitting (argument types via pure
            // typing, which shares `check_call` with `expr_type`), then emit
            // each argument in order and the gosub itself.
            let (id, result) = check_call(name, args, symbols, &|arg| {
                expr_type(
                    arg,
                    &|local| slots.get(local).map(|slot| slot.0),
                    symbols,
                    configs,
                )
            })
            .map_err(NativeError::Invalid)?;
            for arg in args {
                lower_expr(arg, slots, book, symbols, configs, out)?;
            }
            // The gosub is not a `Pure` effect, so it bypasses `emit()` (which
            // would reject it): the opcode still comes from the book, and the
            // operand is the resolved callee id, never a placeholder.
            let opcode = book.opcode_for("gosub_with_params")?;
            out.push(Instruction {
                opcode,
                command: "gosub_with_params".to_string(),
                operand: Operand::Script(id),
            });
            Ok(result)
        }
    }
}

/// Validate a call's name, argument multiset, and return usability, then hand
/// back the callee id and the single result type. Shared by typing and
/// lowering so both agree: `resolve` computes one argument's stack type and
/// reports failures as plain strings in the caller's error shape.
fn check_call(
    name: &str,
    args: &[Expr],
    symbols: &SymbolRegistry,
    resolve: &dyn Fn(&Expr) -> std::result::Result<ValType, String>,
) -> std::result::Result<(i32, ValType), String> {
    let id = symbols
        .resolve_call(name)
        .ok_or_else(|| format!("unknown script '~{name}'"))?;
    let sig = symbols
        .args_of(id)
        .ok_or_else(|| format!("unknown script '~{name}'"))?;
    let mut counts = [0_usize; 3];
    let mut arguments = crate::dataflow::EntryArguments::default();
    for arg in args {
        let lane = match resolve(arg)? {
            ValType::Int => 0,
            ValType::String => 1,
            ValType::Long => 2,
        };
        counts[lane] += 1;
        use crate::dataflow::Constant;
        let constant = match arg {
            Expr::LitInt(value) => Some(Constant::Int(*value)),
            Expr::LitStr(value) => Some(Constant::String(value.clone())),
            Expr::LitLong(value) => Some(Constant::Long(*value)),
            Expr::Component { iface, child } => {
                pack_component_ref(*iface, *child).map(Constant::Int)
            }
            _ => None,
        };
        arguments[lane].push(constant);
    }
    let [ints, objs, longs] = counts;
    if ints != usize::from(sig.int)
        || objs != usize::from(sig.obj)
        || longs != usize::from(sig.long)
    {
        return Err(format!(
            "'~{name}' expects ({}i,{}o,{}l), got ({ints}i,{objs}o,{longs}l)",
            sig.int, sig.obj, sig.long
        ));
    }
    let returns = symbols.returns_for_call(id, &arguments).ok_or_else(|| {
        format!("cannot use '~{name}' as a value: return arity is statically unknowable")
    })?;
    single_value(returns).ok_or_else(|| {
        format!(
            "cannot use '~{name}' as a value: it returns ({}i,{}o,{}l), exactly one value required",
            returns.int, returns.obj, returns.long
        )
    }).map(|ty| (id, ty))
}

/// A return-arity triple usable as one expression value, if it holds exactly
/// one value. Anything else (void, multi-value) cannot sit in expression
/// position. Shared with [`crate::source`] so dump-side value-call lifting
/// and human-side typing agree on what "one value" means.
pub(crate) fn single_value(counts: Counts) -> Option<ValType> {
    match (counts.int, counts.obj, counts.long) {
        (1, 0, 0) => Some(ValType::Int),
        (0, 1, 0) => Some(ValType::String),
        (0, 0, 1) => Some(ValType::Long),
        _ => None,
    }
}

/// Emit one canonical instruction: opcode id from the book (unknown command
/// = error, never a placeholder), cross-checked against the shared effect
/// contract before it lands in `out`.
fn emit(
    book: &OpcodeBook,
    out: &mut Vec<Instruction>,
    command: &str,
    operand: Operand,
    pops: [u16; 3],
    pushes: [u16; 3],
) -> Result<()> {
    let opcode = book.opcode_for(command)?;
    if effect(command, &operand) != (Effect::Fixed { pops, pushes }) {
        return Err(NativeError::Invalid(format!(
            "effect drift for '{command}': shared contract disagrees with the expression table"
        )));
    }
    out.push(Instruction {
        opcode,
        command: command.to_string(),
        operand,
    });
    Ok(())
}

/// Emit one config-typed read (`struct_param` and siblings, `_enum`): opcode
/// id from the book with the canonical `Byte(0)` operand. The shared effect
/// contract is `Unknown` for these by design (it carries no config tables),
/// so there is no `Pure` row to cross-check — instead the pops triple is the
/// handler-verified fixed count (2 for the param family, 4 for `_enum`) and
/// the pushes triple is the single resolved lane from [`pushes_for`]. Any
/// other command or operand here is a loud error, never a guessed byte.
fn emit_config(
    book: &OpcodeBook,
    out: &mut Vec<Instruction>,
    command: &str,
    pops: [u16; 3],
    pushes: [u16; 3],
) -> Result<()> {
    if ParamOp::from_command(command).is_some() {
        if pops != [2, 0, 0] {
            return Err(NativeError::Invalid(format!(
                "operator '{command}' must pop exactly two ints"
            )));
        }
    } else if command == "_enum" {
        if pops != [4, 0, 0] {
            return Err(NativeError::Invalid(
                "operator '_enum' must pop exactly four ints".to_string(),
            ));
        }
    } else {
        return Err(NativeError::Invalid(format!(
            "operator '{command}' is not a config-typed expression operator"
        )));
    }
    let total: u16 = pushes.iter().sum();
    if total != 1 {
        return Err(NativeError::Invalid(format!(
            "operator '{command}' must push exactly one value"
        )));
    }
    let opcode = book.opcode_for(command)?;
    out.push(Instruction {
        opcode,
        command: command.to_string(),
        operand: Operand::Byte(0),
    });
    Ok(())
}

/// Render an expression in surface syntax: infix `+`/`*`/`/`/`%` with the
/// documented precedence table (parenthesizing only when needed), every
/// other operator in `name(args)` function form. Component refs render
/// `Bank/7`-style whenever `inames` names the interface, else the packed
/// decimal — so the output always reparses to an equal model under the same
/// registry.
pub fn format_expr(expr: &Expr, inames: &InterfaceRegistry) -> String {
    match expr {
        Expr::LitInt(value) => value.to_string(),
        Expr::LitLong(value) => format!("{value}L"),
        Expr::Component { iface, child } => inames.display_component_ref(*iface, *child),
        Expr::LitStr(text) => format!(
            "\"{}\"",
            text.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        ),
        Expr::Local(name) => format!("${name}"),
        Expr::Call { name, args } => {
            let mut out = format!("~{name}(");
            for (position, arg) in args.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_expr(arg, inames));
            }
            out.push(')');
            out
        }
        Expr::Unary(op, inner) => {
            format!("{}({})", op.command(), format_expr(inner, inames))
        }
        Expr::Nary(op, args) => {
            let mut out = format!("{}(", op.command());
            for (position, arg) in args.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_expr(arg, inames));
            }
            out.push(')');
            out
        }
        Expr::Join(parts) => {
            let mut out = String::from("join_string(");
            for (position, part) in parts.iter().enumerate() {
                if position > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_expr(part, inames));
            }
            out.push(')');
            out
        }
        Expr::Param { op, obj, param } => {
            format!("{}({}, {param})", op.command(), format_expr(obj, inames))
        }
        Expr::Enum {
            input,
            output,
            enumeration,
            key,
        } => {
            format!(
                "_enum({}, {}, {enumeration}, {})",
                format_expr(input, inames),
                format_expr(output, inames),
                format_expr(key, inames)
            )
        }
        Expr::Binary(op, left, right) => match infix_prec(*op) {
            Some(prec) => {
                let symbol = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Multiply => "*",
                    BinaryOp::Divide => "/",
                    BinaryOp::Modulo => "%",
                    BinaryOp::Min
                    | BinaryOp::Max
                    | BinaryOp::And
                    | BinaryOp::Or
                    | BinaryOp::AddPercent
                    | BinaryOp::SetBit
                    | BinaryOp::ClearBit
                    | BinaryOp::TestBit
                    | BinaryOp::Pow
                    | BinaryOp::QuickchatDynamicCommandAdd
                    | BinaryOp::Append
                    | BinaryOp::Compare => "?",
                };
                let render = |side: &Expr, on_right: bool| {
                    let text = format_expr(side, inames);
                    let need = expr_prec(side);
                    if need < prec || (on_right && need == prec) {
                        format!("({text})")
                    } else {
                        text
                    }
                };
                format!("{} {symbol} {}", render(left, false), render(right, true))
            }
            None => {
                format!(
                    "{}({}, {})",
                    op.command(),
                    format_expr(left, inames),
                    format_expr(right, inames)
                )
            }
        },
    }
}

/// Infix precedence level when the operator has infix syntax (`+` = 1,
/// `*`/`/`/`%` = 2), else `None` (function form is atomic).
fn infix_prec(op: BinaryOp) -> Option<u8> {
    match op {
        BinaryOp::Add => Some(1),
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => Some(2),
        BinaryOp::Min
        | BinaryOp::Max
        | BinaryOp::And
        | BinaryOp::Or
        | BinaryOp::AddPercent
        | BinaryOp::SetBit
        | BinaryOp::ClearBit
        | BinaryOp::TestBit
        | BinaryOp::Pow
        | BinaryOp::QuickchatDynamicCommandAdd
        | BinaryOp::Append
        | BinaryOp::Compare => None,
    }
}

/// The precedence level an expression formats at: infix operators at their
/// level, everything else atomic (3). Drives minimal parenthesization.
fn expr_prec(expr: &Expr) -> u8 {
    match expr {
        Expr::Binary(op, _, _) => infix_prec(*op).unwrap_or(3),
        Expr::LitInt(_)
        | Expr::LitLong(_)
        | Expr::Component { .. }
        | Expr::LitStr(_)
        | Expr::Local(_)
        | Expr::Unary(_, _)
        | Expr::Nary(_, _)
        | Expr::Join(_)
        | Expr::Param { .. }
        | Expr::Enum { .. }
        | Expr::Call { .. } => 3,
    }
}

/// Parse one expression (the right-hand side of `$name = <expr>;`, without
/// the trailing `;`). Recursive descent over literals, `$locals`, component
/// refs, unary, binary, and n-ary operators, and nesting; string-aware
/// throughout (commas and parens inside `"..."` are data — the lexer masks
/// them, so no separate top-level splitting pass is needed). `inames`
/// resolves `Bank/7`-style component refs against the roster (unknown names,
/// unknown children, and empty registries fail loudly); a `/` between two
/// numbers is always division, never a component. Failures are message-only;
/// the line parser wraps them with line numbers. On ANY doubt (unknown
/// operators, `~calls` in value position, bare `-`, arity slips) this rejects
/// loudly.
pub fn parse_expr(text: &str, inames: &InterfaceRegistry) -> std::result::Result<Expr, String> {
    let tokens = lex(text)?;
    let mut parser = TokenParser {
        tokens: &tokens,
        pos: 0,
        depth: 0,
        inames,
    };
    let expr = parser.parse_add()?;
    match parser.peek() {
        Tok::Eof => Ok(expr),
        other => Err(format!("unexpected {other} after expression")),
    }
}

/// One lexed token. Numbers arrive value-checked (out-of-range literals fail
/// in the lexer); strings arrive unescaped.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Tok {
    /// Value-checked i32 literal (glued `-` folds into the value).
    Int(i32),
    /// Value-checked i64 literal (`41L`).
    Long(i64),
    /// Unescaped string contents.
    Str(String),
    /// `$`-less local name.
    Local(String),
    /// Bare identifier (only valid as a call head).
    Ident(String),
    /// `+`.
    Plus,
    /// Stray `-` (the lexer folds `-<digit>` into numbers; what remains is
    /// never a binary operator — there is no subtract command).
    Minus,
    /// `*`.
    Star,
    /// `/`.
    Slash,
    /// `%`.
    Percent,
    /// `(`.
    LParen,
    /// `)`.
    RParen,
    /// `,`.
    Comma,
    /// `~` — a call where a value must be; always rejected with the
    /// calls-as-values message.
    Tilde,
    /// End of input (always appended by the lexer).
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(_) => write!(formatter, "integer"),
            Self::Long(_) => write!(formatter, "long"),
            Self::Str(_) => write!(formatter, "string"),
            Self::Local(name) => write!(formatter, "'${name}'"),
            Self::Ident(name) => write!(formatter, "name '{name}'"),
            Self::Plus => write!(formatter, "'+'"),
            Self::Minus => write!(formatter, "'-'"),
            Self::Star => write!(formatter, "'*'"),
            Self::Slash => write!(formatter, "'/'"),
            Self::Percent => write!(formatter, "'%'"),
            Self::LParen => write!(formatter, "'('"),
            Self::RParen => write!(formatter, "')'"),
            Self::Comma => write!(formatter, "','"),
            Self::Tilde => write!(formatter, "'~'"),
            Self::Eof => write!(formatter, "end of expression"),
        }
    }
}

/// Lex an expression into tokens. Strings mask commas and parens (the shared
/// call-arg splitting discipline, handled here by construction); a `-`
/// glued to a digit folds into a negative literal.
fn lex(text: &str) -> std::result::Result<Vec<Tok>, String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0_usize;
    while pos < bytes.len() {
        let byte = bytes[pos];
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' => {
                pos += 1;
            }
            b'+' => {
                tokens.push(Tok::Plus);
                pos += 1;
            }
            b'*' => {
                tokens.push(Tok::Star);
                pos += 1;
            }
            b'/' => {
                tokens.push(Tok::Slash);
                pos += 1;
            }
            b'%' => {
                tokens.push(Tok::Percent);
                pos += 1;
            }
            b'(' => {
                tokens.push(Tok::LParen);
                pos += 1;
            }
            b')' => {
                tokens.push(Tok::RParen);
                pos += 1;
            }
            b',' => {
                tokens.push(Tok::Comma);
                pos += 1;
            }
            b'~' => {
                tokens.push(Tok::Tilde);
                pos += 1;
            }
            b'-' if bytes.get(pos + 1).is_some_and(u8::is_ascii_digit) => {
                let (token, next) = lex_number(bytes, pos)?;
                tokens.push(token);
                pos = next;
            }
            b'-' => {
                tokens.push(Tok::Minus);
                pos += 1;
            }
            b'"' => {
                let (text, next) = lex_string(bytes, pos)?;
                tokens.push(Tok::Str(text));
                pos = next;
            }
            b'$' => {
                let (name, next) = lex_local(text, bytes, pos)?;
                tokens.push(Tok::Local(name));
                pos = next;
            }
            byte if byte.is_ascii_digit() => {
                let (token, next) = lex_number(bytes, pos)?;
                tokens.push(token);
                pos = next;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = pos;
                while pos < bytes.len()
                    && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_')
                {
                    pos += 1;
                }
                tokens.push(Tok::Ident(text[start..pos].to_string()));
            }
            _ => {
                let char = text[pos..].chars().next().unwrap_or('?');
                return Err(format!("unexpected character '{char}' in expression"));
            }
        }
    }
    tokens.push(Tok::Eof);
    Ok(tokens)
}

/// Lex a (possibly `-`-glued) `digits[L]` number at `pos`. Magnitudes parse
/// wide and range-check with the sign applied, so `-2147483648` and
/// `-9223372036854775808L` are expressible while their positive mirrors fail.
fn lex_number(bytes: &[u8], pos: usize) -> std::result::Result<(Tok, usize), String> {
    let mut cursor = pos;
    let negative = if bytes[cursor] == b'-' {
        cursor += 1;
        true
    } else {
        false
    };
    let start = cursor;
    let mut magnitude: u64 = 0;
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        let digit = u64::from(bytes[cursor] - b'0');
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit))
            .ok_or_else(|| "integer literal out of range".to_string())?;
        cursor += 1;
    }
    if cursor == start {
        return Err("expected a number".to_string());
    }
    let is_long = bytes.get(cursor) == Some(&b'L');
    if is_long {
        cursor += 1;
    }
    if is_long {
        const MAX_PLUS_1: u64 = i64::MAX as u64 + 1;
        if magnitude > MAX_PLUS_1 || (!negative && magnitude == MAX_PLUS_1) {
            return Err("integer literal out of range".to_string());
        }
        let value = if negative && magnitude == MAX_PLUS_1 {
            i64::MIN
        } else if negative {
            -(magnitude as i64)
        } else {
            magnitude as i64
        };
        Ok((Tok::Long(value), cursor))
    } else {
        const MAX_PLUS_1: u64 = i32::MAX as u64 + 1;
        if magnitude > MAX_PLUS_1 || (!negative && magnitude == MAX_PLUS_1) {
            return Err("integer literal out of range".to_string());
        }
        let value = if negative && magnitude == MAX_PLUS_1 {
            i32::MIN
        } else if negative {
            -(magnitude as i32)
        } else {
            magnitude as i32
        };
        Ok((Tok::Int(value), cursor))
    }
}

/// Lex a `"..."` string at `pos` (the opening quote): same `\\ \" \n \r \t`
/// escapes as source strings, raw bytes otherwise (UTF-8 passes through —
/// quotes and backslashes are single-byte, so byte scanning is safe).
fn lex_string(bytes: &[u8], pos: usize) -> std::result::Result<(String, usize), String> {
    let mut out: Vec<u8> = Vec::new();
    let mut cursor = pos + 1;
    loop {
        let Some(byte) = bytes.get(cursor).copied() else {
            return Err("unterminated string literal".to_string());
        };
        cursor += 1;
        match byte {
            b'"' => {
                let text = String::from_utf8(out).map_err(|_| "bad string literal".to_string())?;
                return Ok((text, cursor));
            }
            b'\\' => {
                let Some(escaped) = bytes.get(cursor).copied() else {
                    return Err("dangling escape in string literal".to_string());
                };
                cursor += 1;
                match escaped {
                    b'\\' => out.push(b'\\'),
                    b'"' => out.push(b'"'),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    _ => {
                        return Err(format!(
                            "bad escape '\\{}' in string literal",
                            char::from(escaped)
                        ));
                    }
                }
            }
            _ => out.push(byte),
        }
    }
}

/// Lex a `$name` local at `pos` (the `$`).
fn lex_local(text: &str, bytes: &[u8], pos: usize) -> std::result::Result<(String, usize), String> {
    let mut cursor = pos + 1;
    while cursor < bytes.len() && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_') {
        cursor += 1;
    }
    let name = text[pos + 1..cursor].to_string();
    if !crate::source::is_valid_name(&name) {
        return Err(format!("bad local '${name}' (expected '$name')"));
    }
    Ok((name, cursor))
}

/// Recursive descent over a lexed token stream.
struct TokenParser<'a> {
    tokens: &'a [Tok],
    pos: usize,
    depth: usize,
    inames: &'a InterfaceRegistry,
}

impl TokenParser<'_> {
    fn peek(&self) -> &Tok {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    fn bump(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    /// Nest one `(`/call level, bounded by [`MAX_NESTING`].
    fn enter(&mut self) -> std::result::Result<(), String> {
        if self.depth >= MAX_NESTING {
            return Err("expression is too deeply nested".to_string());
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// `term ("+" term)*` — left-associative.
    fn parse_add(&mut self) -> std::result::Result<Expr, String> {
        let mut left = self.parse_mul()?;
        while matches!(self.peek(), Tok::Plus) {
            self.bump();
            let right = self.parse_mul()?;
            left = Expr::Binary(BinaryOp::Add, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `primary (("*"|"/"|"%") primary)*` — left-associative.
    fn parse_mul(&mut self) -> std::result::Result<Expr, String> {
        let mut left = self.parse_primary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinaryOp::Multiply,
                Tok::Slash => BinaryOp::Divide,
                Tok::Percent => BinaryOp::Modulo,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.parse_primary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    /// Literals, `$locals`, component refs, calls, `(expr)`, glued-negative
    /// fallback.
    fn parse_primary(&mut self) -> std::result::Result<Expr, String> {
        match self.peek().clone() {
            Tok::Int(value) => {
                self.bump();
                Ok(Expr::LitInt(value))
            }
            Tok::Long(value) => {
                self.bump();
                Ok(Expr::LitLong(value))
            }
            Tok::Str(text) => {
                self.bump();
                Ok(Expr::LitStr(text))
            }
            Tok::Local(name) => {
                self.bump();
                Ok(Expr::Local(name))
            }
            Tok::Ident(name) => {
                self.bump();
                // A bare name followed by `/` is a component ref (`bank/7`):
                // names never start a division (only numbers do), so the
                // shapes cannot collide. Anything else is a call head.
                if matches!(self.peek(), Tok::Slash) {
                    self.bump();
                    self.parse_component(&name)
                } else {
                    self.parse_call(&name)
                }
            }
            Tok::LParen => {
                self.bump();
                self.enter()?;
                let expr = self.parse_add()?;
                self.leave();
                match self.peek() {
                    Tok::RParen => {
                        self.bump();
                        Ok(expr)
                    }
                    _ => Err("missing ')' in expression".to_string()),
                }
            }
            Tok::Minus => {
                // The lexer folds every glued `-<digit>` into a literal, so
                // a Minus here is spaced or applied to a non-number: only a
                // literal negation is expressible (there is no negate
                // command), everything else fails loudly.
                self.bump();
                match self.peek().clone() {
                    Tok::Int(value) => {
                        self.bump();
                        value
                            .checked_neg()
                            .map(Expr::LitInt)
                            .ok_or_else(|| "integer literal out of range".to_string())
                    }
                    Tok::Long(value) => {
                        self.bump();
                        value
                            .checked_neg()
                            .map(Expr::LitLong)
                            .ok_or_else(|| "integer literal out of range".to_string())
                    }
                    _ => Err(
                        "unary '-' applies to number literals only (there is no negate command)"
                            .to_string(),
                    ),
                }
            }
            Tok::Tilde => {
                self.bump();
                self.parse_tilde_call()
            }
            other => Err(format!(
                "unexpected {other} (expected a value, $local, or operator)"
            )),
        }
    }

    /// `Bank/7` after the head name and `/` were consumed: a packed component
    /// reference. The child is a curated name or a roster-checked number;
    /// unknown interfaces, unknown children, and empty registries all fail
    /// loudly (never guessed, never division — division needs numbers on both
    /// sides, and the head here is a name).
    fn parse_component(&mut self, iface: &str) -> std::result::Result<Expr, String> {
        let child_text = match self.peek().clone() {
            Tok::Int(value) => {
                self.bump();
                value.to_string()
            }
            Tok::Ident(name) => {
                self.bump();
                name
            }
            other => {
                return Err(format!(
                    "expected a component child after '{iface}/', got {other}"
                ));
            }
        };
        match self.inames.resolve_component(iface, &child_text) {
            Some((iface_id, child)) => Ok(Expr::Component {
                iface: iface_id,
                child,
            }),
            None if self.inames.resolve_iface(iface).is_none() => {
                Err(format!("unknown interface '{iface}'"))
            }
            None => Err(format!("unknown component '{iface}/{child_text}'")),
        }
    }

    /// `~name(args)` after the `~` was consumed: a call used as a value.
    /// Argument expressions parse recursively (calls nest freely); the name
    /// is recorded unresolved — typing and lowering resolve it against the
    /// registry, so an unknown name fails there, never here.
    fn parse_tilde_call(&mut self) -> std::result::Result<Expr, String> {
        let name = match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                name
            }
            other => {
                return Err(format!("expected a script name after '~', got {other}"));
            }
        };
        if !matches!(self.peek(), Tok::LParen) {
            return Err(format!("expected '(' after '~{name}'"));
        }
        self.bump();
        let mut args = Vec::new();
        if matches!(self.peek(), Tok::RParen) {
            self.bump();
        } else {
            loop {
                self.enter()?;
                let arg = self.parse_add()?;
                self.leave();
                args.push(arg);
                match self.peek() {
                    Tok::Comma => {
                        self.bump();
                    }
                    Tok::RParen => {
                        self.bump();
                        break;
                    }
                    other => {
                        return Err(format!("expected ',' or ')' in '~{name}(...', got {other}"));
                    }
                }
            }
        }
        Ok(Expr::Call { name, args })
    }

    /// `name(args)` after the head `name` was consumed: exactly the verified
    /// operator set, with exact arities (`join_string` is variadic — 2+
    /// parts, and the part count becomes its explicit count operand).
    /// Unknown names (including real but out-of-scope commands) fail loudly.
    fn parse_call(&mut self, name: &str) -> std::result::Result<Expr, String> {
        if !matches!(self.peek(), Tok::LParen) {
            return Err(format!(
                "unexpected name '{name}' (expression operators are functions: min(a, b), not(x))"
            ));
        }
        self.bump();
        let mut args = Vec::new();
        if matches!(self.peek(), Tok::RParen) {
            self.bump();
        } else {
            loop {
                self.enter()?;
                let arg = self.parse_add()?;
                self.leave();
                args.push(arg);
                match self.peek() {
                    Tok::Comma => {
                        self.bump();
                    }
                    Tok::RParen => {
                        self.bump();
                        break;
                    }
                    other => {
                        return Err(format!("expected ',' or ')' in '{name}(...', got {other}"));
                    }
                }
            }
        }
        build_call(name, args)
    }
}

/// Assemble a call head plus evaluated arguments into an operator node:
/// exactly the verified set, with exact arities (`join_string` is variadic —
/// 2+ parts, and the part count becomes its explicit count operand).
fn build_call(name: &str, args: Vec<Expr>) -> std::result::Result<Expr, String> {
    let got = args.len();
    let mut items = args.into_iter();
    match name {
        "min"
        | "max"
        | "and"
        | "or"
        | "addpercent"
        | "setbit"
        | "clearbit"
        | "testbit"
        | "pow"
        | "quickchat_dynamic_command_add"
        | "append"
        | "compare" => {
            let (Some(left), Some(right), None) = (items.next(), items.next(), items.next()) else {
                return Err(format!("operator '{name}' takes 2 arguments, got {got}"));
            };
            let op = match name {
                "min" => BinaryOp::Min,
                "max" => BinaryOp::Max,
                "and" => BinaryOp::And,
                "or" => BinaryOp::Or,
                "addpercent" => BinaryOp::AddPercent,
                "setbit" => BinaryOp::SetBit,
                "clearbit" => BinaryOp::ClearBit,
                "testbit" => BinaryOp::TestBit,
                "pow" => BinaryOp::Pow,
                "quickchat_dynamic_command_add" => BinaryOp::QuickchatDynamicCommandAdd,
                "append" => BinaryOp::Append,
                _ => BinaryOp::Compare,
            };
            Ok(Expr::Binary(op, Box::new(left), Box::new(right)))
        }
        "not" | "random" | "randominc" | "string_length" | "tostring" => {
            let (Some(only), None) = (items.next(), items.next()) else {
                return Err(format!("operator '{name}' takes 1 argument, got {got}"));
            };
            let op = match name {
                "not" => UnaryOp::Not,
                "random" => UnaryOp::Random,
                "randominc" => UnaryOp::RandomInc,
                "string_length" => UnaryOp::StringLength,
                _ => UnaryOp::ToString,
            };
            Ok(Expr::Unary(op, Box::new(only)))
        }
        "scale" | "interpolate" => {
            let op = match name {
                "scale" => NaryOp::Scale,
                _ => NaryOp::Interpolate,
            };
            if got != op.arity() {
                return Err(format!(
                    "operator '{name}' takes {} arguments, got {got}",
                    op.arity()
                ));
            }
            Ok(Expr::Nary(op, items.collect()))
        }
        "join_string" => {
            // Variadic: the written count IS the arity, tied to the explicit
            // count operand at every later stage (see `join_shape`).
            if got < MIN_JOIN_ARITY {
                return Err(format!(
                    "operator 'join_string' takes at least {MIN_JOIN_ARITY} arguments, got {got}"
                ));
            }
            Ok(Expr::Join(items.collect()))
        }
        "struct_param" | "oc_param" | "nc_param" | "lc_param" | "seq_param" => {
            let (Some(obj), Some(param), None) = (items.next(), items.next(), items.next()) else {
                return Err(format!("operator '{name}' takes 2 arguments, got {got}"));
            };
            let Expr::LitInt(param) = param else {
                return Err(format!(
                    "operator '{name}' needs a literal param id as its second argument"
                ));
            };
            let op =
                ParamOp::from_command(name).ok_or_else(|| format!("unknown operator '{name}'"))?;
            Ok(Expr::Param {
                op,
                obj: Box::new(obj),
                param,
            })
        }
        "_enum" => {
            if got != 4 {
                return Err(format!("operator '_enum' takes 4 arguments, got {got}"));
            }
            let mut items = items;
            let (Some(input), Some(output), Some(enumeration), Some(key)) =
                (items.next(), items.next(), items.next(), items.next())
            else {
                return Err(format!("operator '_enum' takes 4 arguments, got {got}"));
            };
            let Expr::LitInt(enumeration) = enumeration else {
                return Err(
                    "operator '_enum' needs a literal enum id as its third argument".to_string(),
                );
            };
            Ok(Expr::Enum {
                input: Box::new(input),
                output: Box::new(output),
                enumeration,
                key: Box::new(key),
            })
        }
        _ => Err(format!(
            "unknown operator '{name}' (expected min, max, and, or, addpercent, setbit, clearbit, testbit, pow, quickchat_dynamic_command_add, append, compare, join_string, not, random, randominc, string_length, tostring, scale, interpolate, struct_param, oc_param, nc_param, lc_param, seq_param, _enum)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> OpcodeBook {
        OpcodeBook::embedded().unwrap()
    }

    fn slots() -> BTreeMap<&'static str, (ValType, i32)> {
        BTreeMap::from([
            ("a", (ValType::Int, 0)),
            ("b", (ValType::Int, 2)),
            ("y", (ValType::Int, 1)),
            ("s", (ValType::String, 0)),
            ("t", (ValType::String, 1)),
            ("l", (ValType::Long, 0)),
        ])
    }

    fn configs() -> ConfigTypes {
        ConfigTypes::empty()
    }

    fn inames() -> crate::inames::InterfaceRegistry {
        crate::inames::InterfaceRegistry::empty()
    }

    #[test]
    fn precedence_parses_and_formats_minimally() {
        // `*` binds tighter than `+`.
        let expr = parse_expr("$a + $b * 2", &inames()).unwrap();
        assert_eq!(
            expr,
            Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Local("a".to_string())),
                Box::new(Expr::Binary(
                    BinaryOp::Multiply,
                    Box::new(Expr::Local("b".to_string())),
                    Box::new(Expr::LitInt(2)),
                )),
            )
        );
        assert_eq!(format_expr(&expr, &inames()), "$a + $b * 2");
        // Left nesting needs no parens (left-assoc reparse is identical).
        let left = parse_expr("($a + $b) + $c", &inames()).unwrap();
        assert_eq!(format_expr(&left, &inames()), "$a + $b + $c");
        assert_eq!(
            parse_expr(&format_expr(&left, &inames()), &inames()).unwrap(),
            left
        );
        // Right nesting keeps its parens (dropping them would reshape).
        let right = parse_expr("$a + ($b + $c)", &inames()).unwrap();
        assert_eq!(format_expr(&right, &inames()), "$a + ($b + $c)");
        assert_eq!(
            parse_expr(&format_expr(&right, &inames()), &inames()).unwrap(),
            right
        );
        // Same-level `/` under `*` on the right keeps parens.
        let mixed = parse_expr("$a * ($b / $c)", &inames()).unwrap();
        assert_eq!(format_expr(&mixed, &inames()), "$a * ($b / $c)");
        // A lower-precedence left operand keeps its parens: another tree.
        let grouped = parse_expr("($a + $b) * 2", &inames()).unwrap();
        assert_ne!(grouped, expr);
        assert_eq!(format_expr(&grouped, &inames()), "($a + $b) * 2");
        // A unary node keeps its operand grouped.
        assert_eq!(
            parse_expr("not($a)", &inames()).unwrap(),
            Expr::Unary(UnaryOp::Not, Box::new(Expr::Local("a".to_string())))
        );
    }

    #[test]
    fn literals_cover_edges() {
        assert_eq!(
            parse_expr("-2147483648", &inames()).unwrap(),
            Expr::LitInt(i32::MIN)
        );
        assert_eq!(
            parse_expr("-9223372036854775808L", &inames()).unwrap(),
            Expr::LitLong(i64::MIN)
        );
        assert_eq!(parse_expr("- 5", &inames()).unwrap(), Expr::LitInt(-5));
        assert!(parse_expr("2147483648", &inames()).is_err());
        assert!(parse_expr("9223372036854775808L", &inames()).is_err());
        assert_eq!(
            parse_expr("\"a, (b\"", &inames()).unwrap(),
            Expr::LitStr("a, (b".to_string())
        );
    }

    fn call_registry() -> SymbolRegistry {
        let mut known = BTreeMap::new();
        known.insert(
            100,
            Counts {
                int: 2,
                obj: 0,
                long: 0,
            },
        );
        known.insert(
            101,
            Counts {
                int: 0,
                obj: 1,
                long: 0,
            },
        );
        known.insert(102, Counts::default());
        let mut returns = BTreeMap::new();
        returns.insert(
            100,
            Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
        );
        returns.insert(
            101,
            Counts {
                int: 0,
                obj: 1,
                long: 0,
            },
        );
        SymbolRegistry::build(
            vec![
                (100, "gives_int".to_string()),
                (101, "gives_str".to_string()),
                (102, "takes_nothing".to_string()),
            ],
            &known,
        )
        .unwrap()
        .with_returns(&returns)
    }

    #[test]
    fn calls_parse_lower_and_format() {
        let book = book();
        let slots = slots();
        let symbols = call_registry();

        // Parse shape, then format stability.
        let expr = parse_expr("~gives_int($a, 41)", &inames()).unwrap();
        assert_eq!(
            expr,
            Expr::Call {
                name: "gives_int".to_string(),
                args: vec![Expr::Local("a".to_string()), Expr::LitInt(41),],
            }
        );
        assert_eq!(format_expr(&expr, &inames()), "~gives_int($a, 41)");
        assert_eq!(
            parse_expr(&format_expr(&expr, &inames()), &inames()).unwrap(),
            expr
        );

        // Typing follows the single inferred return.
        let ty = |expr: &Expr| {
            expr_type(
                expr,
                &|name| slots.get(name).map(|slot| slot.0),
                &symbols,
                &configs(),
            )
        };
        assert_eq!(ty(&expr).unwrap(), ValType::Int);
        assert_eq!(
            ty(&parse_expr("~gives_str(\"x\")", &inames()).unwrap()).unwrap(),
            ValType::String
        );

        // Lowering emits arguments in order plus the resolved gosub.
        let mut out = Vec::new();
        let result = lower_expr(&expr, &slots, &book, &symbols, &configs(), &mut out).unwrap();
        assert_eq!(result, ValType::Int);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].command, "push_int_local");
        assert_eq!(out[1].command, "push_constant_string");
        assert_eq!(out[2].command, "gosub_with_params");
        assert_eq!(out[2].opcode, book.opcode_for("gosub_with_params").unwrap());
        assert!(matches!(
            out[2].operand,
            crate::script::Operand::Script(100)
        ));

        // Nested calls lower inside-out.
        let nested = parse_expr("~gives_int(~gives_int($a, 1), 2)", &inames()).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            lower_expr(&nested, &slots, &book, &symbols, &configs(), &mut out).unwrap(),
            ValType::Int
        );
        assert_eq!(out.len(), 5);
        assert_eq!(out[2].command, "gosub_with_params");
        assert_eq!(out[4].command, "gosub_with_params");
    }
}
