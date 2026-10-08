//! Line parser for native source text into [`SourceScript`](crate::source::SourceScript).
//!
//! Hand-written, one line at a time, so every failure names its line. `//`
//! comments run to end of line (a `//` inside a string literal is data, not a
//! comment); blank lines are insignificant. The parser checks syntax only —
//! duplicate labels and parameters are rejected here with lines, while slot
//! assignment, undefined labels, and undeclared locals belong to
//! [`lower`](crate::source::lower), which owns the whole-script view.
//!
//! Packed component ids spell symbolically (`bank/7`, `bank/main`) in the
//! int-literal slots of `push_constant_string`/`push_constant_int` and in
//! expression `LitInt` positions, validated against the interface roster at
//! parse (see [`InterfaceRegistry`](crate::inames::InterfaceRegistry)):
//! unknown names, unknown children, and empty registries all fail with lines.
//! The shape is NAME-led only, so `1253/4` in an expression stays division.

use crate::error::NativeError;
use crate::inames::InterfaceRegistry;
use crate::script::is_branch_command;
use crate::source::{
    CallArg, Param, SourceInstr, SourceOperand, SourceScript, SourceStmt, ValType, is_valid_name,
};
use crate::symbols::SymbolRegistry;
use crate::vars::VarScope;

/// A parse failure at one line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    /// What was wrong.
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

impl From<ParseError> for NativeError {
    fn from(error: ParseError) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// Commands taking a local slot operand, by value type.
const INT_LOCAL_COMMANDS: &[&str] = &["push_int_local", "pop_int_local"];
const STRING_LOCAL_COMMANDS: &[&str] = &["push_string_local", "pop_string_local"];
const LONG_LOCAL_COMMANDS: &[&str] = &["push_long_local", "pop_long_local"];
/// Commands taking an array id.
const ARRAY_COMMANDS: &[&str] = &[
    "define_array",
    "push_array_int",
    "pop_array_int",
    "push_array_int_leave_index_on_stack",
    "push_array_int_and_index",
    "pop_array_int_leave_value_on_stack",
];

/// Parse source text with `~calls` resolved through `symbols` and
/// config-typed reads (`struct_param` and siblings, `_enum`) resolved through
/// `configs`. An empty registry parses numeric-only source; a populated one
/// also accepts `~name();` (empty parens — call arguments arrive with the
/// signature-aware call syntax, not here). An empty [`ConfigTypes`] rejects
/// every config-typed read as unknown — never guessed.
///
/// `inames` resolves symbolic component refs (`bank/7`, `bank/main`) against
/// the interface roster: unknown names, unknown children, and empty
/// registries fail with lines, while numeric text parses unaffected with or
/// without pack data.
pub fn parse_source(
    text: &str,
    symbols: &SymbolRegistry,
    configs: &crate::config::ConfigTypes,
    inames: &InterfaceRegistry,
) -> std::result::Result<SourceScript, ParseError> {
    Ok(parse_source_mapped(text, symbols, configs, inames)?.0)
}

/// Parse with one-based source lines for every body statement, including labels.
pub fn parse_source_mapped(
    text: &str,
    symbols: &SymbolRegistry,
    configs: &crate::config::ConfigTypes,
    inames: &InterfaceRegistry,
) -> std::result::Result<(SourceScript, Vec<usize>), ParseError> {
    let mut source_lines = Vec::new();
    let mut switch_start = 0;
    let mut parser = Parser {
        symbols,
        configs,
        inames,
        line_no: 0,
        header_done: false,
        name: None,
        args: Vec::new(),
        locals: Vec::new(),
        body: Vec::new(),
        labels: Vec::new(),
        switch_cases: None,
    };
    for (position, raw) in text.lines().enumerate() {
        parser.line_no = position + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let code = strip_comment(line);
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        let before = parser.body.len();
        if parser.switch_cases.is_none() {
            switch_start = parser.line_no;
        }
        parser.parse_line(code)?;
        source_lines.extend(std::iter::repeat_n(
            switch_start,
            parser.body.len() - before,
        ));
    }
    Ok((parser.finish()?, source_lines))
}

struct Parser<'a> {
    symbols: &'a SymbolRegistry,
    configs: &'a crate::config::ConfigTypes,
    inames: &'a InterfaceRegistry,
    line_no: usize,
    header_done: bool,
    name: Option<String>,
    args: Vec<Param>,
    locals: Vec<Param>,
    body: Vec<SourceStmt>,
    labels: Vec<(String, usize)>,
    switch_cases: Option<Vec<(i32, String)>>,
}

impl Parser<'_> {
    fn parse_line(&mut self, code: &str) -> std::result::Result<(), ParseError> {
        if self.switch_cases.is_some() {
            return self.parse_switch_line(code);
        }
        if !self.header_done {
            self.parse_header(code)?;
            self.header_done = true;
            return Ok(());
        }
        if let Some(call) = code.strip_prefix('~') {
            self.body.push(SourceStmt::Instr(self.parse_call(call)?));
            return Ok(());
        }
        if let Some(decl) = parse_decl(code) {
            if self.body_started() {
                return Err(self.bad(
                    "declarations must precede the body (canonical form keeps them up top)"
                        .to_string(),
                ));
            }
            let (ty, names) = decl.map_err(|message| self.bad(message))?;
            for name in names {
                self.check_param_fresh(&name)?;
                self.locals.push(Param { ty, name });
            }
            return Ok(());
        }
        // Expression assignments (`$name = <expr>;`) dispatch BEFORE the
        // generic instruction parse: labels and instructions never start
        // with `$`, so the prefix collides with nothing existing. A `$`-led
        // line that is not a valid assignment fails here, never as a
        // misleading instruction error.
        if code.starts_with('$') {
            let (target, expr) = self.parse_assign(code)?;
            self.body.push(SourceStmt::Assign { target, expr });
            return Ok(());
        }
        if let Some(label) = code.strip_suffix(':') {
            let label = label.trim();
            check_ident(label, "label").map_err(|message| self.bad(message))?;
            self.check_label_fresh(label)?;
            self.labels.push((label.to_string(), self.line_no));
            self.body.push(SourceStmt::Label(label.to_string()));
            return Ok(());
        }
        if code == "switch {" {
            self.switch_cases = Some(Vec::new());
            return Ok(());
        }
        let instr = self.parse_instr(code)?;
        self.body.push(SourceStmt::Instr(instr));
        Ok(())
    }

    fn parse_switch_line(&mut self, code: &str) -> std::result::Result<(), ParseError> {
        if code == "}" {
            let cases = self.switch_cases.take().unwrap_or_default();
            // Empty switches exist in the wild (a dispatched-no-op); the
            // binary carries them, so the source does too.
            self.body.push(SourceStmt::Instr(SourceInstr {
                command: "switch".to_string(),
                operand: SourceOperand::Cases(cases),
            }));
            return Ok(());
        }
        let rest = code.strip_prefix("case ").ok_or_else(|| {
            self.bad(format!(
                "expected 'case <int> -> <label>;' or '}}', got '{code}'"
            ))
        })?;
        let (value_text, target_text) = rest.split_once("->").ok_or_else(|| {
            self.bad(format!(
                "malformed case '{code}' (expected 'case <int> -> <label>;')"
            ))
        })?;
        let value: i32 = value_text
            .trim()
            .parse()
            .map_err(|_| self.bad(format!("bad case value '{}'", value_text.trim())))?;
        let target = target_text
            .trim()
            .strip_suffix(';')
            .ok_or_else(|| self.bad(format!("case misses its ';': '{code}'")))?;
        let target = target.trim();
        check_ident(target, "case target").map_err(|message| self.bad(message))?;
        if let Some(cases) = self.switch_cases.as_mut() {
            // Duplicate case values are the author's business (the client
            // matches first-wins); order is preserved exactly.
            cases.push((value, target.to_string()));
        }
        Ok(())
    }

    /// Whether any body statement has been seen (labels, calls, instructions,
    /// switch blocks). Declarations must precede the body so every `$local`
    /// reference below resolves against complete tables with line errors.
    fn body_started(&self) -> bool {
        !self.body.is_empty() || self.switch_cases.is_some()
    }

    fn bad(&self, message: String) -> ParseError {
        ParseError {
            line: self.line_no,
            message,
        }
    }

    fn parse_header(&mut self, code: &str) -> std::result::Result<(), ParseError> {
        let rest = code
            .strip_prefix('[')
            .ok_or_else(|| self.bad(format!("missing script header, got '{code}'")))?;
        let (tag, after) = rest
            .split_once(']')
            .ok_or_else(|| self.bad(format!("unterminated header tag in '{code}'")))?;
        let tag = tag.trim();
        self.name = if tag.is_empty() {
            None
        } else {
            Some(tag.to_string())
        };
        let after = after.trim();
        if after.is_empty() {
            return Ok(());
        }
        let params = after
            .strip_prefix('(')
            .and_then(|inner| inner.strip_suffix(')'))
            .ok_or_else(|| self.bad(format!("malformed argument list in '{code}'")))?;
        if params.trim().is_empty() {
            return Ok(());
        }
        for item in params.split(',') {
            let item = item.trim();
            let (ty_text, name_text) = item.split_once(' ').ok_or_else(|| {
                self.bad(format!(
                    "malformed argument '{item}' (expected 'type $name')"
                ))
            })?;
            let ty = ValType::parse_keyword(ty_text.trim())
                .map_err(|error| self.bad(format!("{error}")))?;
            let name = parse_local_name(name_text.trim())
                .ok_or_else(|| self.bad(format!("malformed argument name in '{item}'")))?;
            self.check_param_fresh(&name)?;
            self.args.push(Param { ty, name });
        }
        Ok(())
    }

    /// `~name(args);` — a call through the registry. Arguments are literals
    /// or `$locals`, validated as a TYPE MULTISET against the callee
    /// signature (cross-type order is free on the client's typed stacks):
    /// every int position's worth of int-typed values, and so on, in any
    /// order. `$local` kinds come from the declarations, which is why they
    /// must precede the body.
    fn parse_call(&self, code: &str) -> std::result::Result<SourceInstr, ParseError> {
        let (name, args) = code.split_once('(').ok_or_else(|| {
            self.bad(format!(
                "malformed call '~{code}' (expected '~name(args);')"
            ))
        })?;
        let name = name.trim();
        if !is_valid_name(name) {
            return Err(self.bad(format!("bad call target '~{name}'")));
        }
        let args = args
            .strip_suffix(");")
            .ok_or_else(|| self.bad(format!("call misses its ');': '~{code}'")))?;
        let id = self
            .symbols
            .resolve_call(name)
            .ok_or_else(|| self.bad(format!("unknown script '~{name}'")))?;
        let sig = self
            .symbols
            .args_of(id)
            .ok_or_else(|| self.bad(format!("unknown script '~{name}'")))?;
        let mut call_args = Vec::new();
        if !args.trim().is_empty() {
            for item in split_call_args(args).map_err(|message| self.bad(message))? {
                call_args.push(self.parse_call_arg(item)?);
            }
        }
        // Multiset check: count kinds, compare against the signature.
        let mut ints = 0_usize;
        let mut objs = 0_usize;
        let mut longs = 0_usize;
        for arg in &call_args {
            match self.call_arg_kind(arg)? {
                ValType::Int => ints += 1,
                ValType::String => objs += 1,
                ValType::Long => longs += 1,
            }
        }
        if ints != sig.int as usize || objs != sig.obj as usize || longs != sig.long as usize {
            return Err(self.bad(format!(
                "'~{name}' expects ({}i,{}o,{}l), got ({ints}i,{objs}o,{longs}l)",
                sig.int, sig.obj, sig.long
            )));
        }
        Ok(SourceInstr {
            command: "gosub_with_params".to_string(),
            operand: SourceOperand::Call(name.to_string(), call_args),
        })
    }

    /// Parse one call argument and resolve `$locals` against declarations
    /// (which precede the body, so the tables are complete here).
    fn parse_call_arg(&self, text: &str) -> std::result::Result<CallArg, ParseError> {
        if let Some(name) = text.strip_prefix('$') {
            if !is_valid_name(name) {
                return Err(self.bad(format!("bad call argument '${name}'")));
            }
            if !self
                .args
                .iter()
                .chain(&self.locals)
                .any(|param| param.name == name)
            {
                return Err(self.bad(format!("undeclared local '${name}'")));
            }
            return Ok(CallArg::Local(name.to_string()));
        }
        if let Some(rest) = text.strip_prefix("varbit:") {
            // Varbits are int-valued by construction; plain vars are not
            // (per-id types live in config data), so only varbits qualify.
            let mut parts = rest.split(':');
            let id: u16 = parts
                .next()
                .unwrap_or("")
                .parse()
                .map_err(|_| self.bad(format!("bad varbit id in call argument '{text}'")))?;
            let transmog = match parts.next() {
                None => false,
                Some("transmog") => true,
                Some(extra) => {
                    return Err(self.bad(format!("bad varbit suffix '{extra}'")));
                }
            };
            if parts.next().is_some() {
                return Err(self.bad(format!("bad varbit call argument '{text}'")));
            }
            return Ok(CallArg::VarBit(id, transmog));
        }
        if let Some(digits) = text.strip_suffix('L') {
            let value: i64 = digits
                .parse()
                .map_err(|_| self.bad(format!("bad call argument '{text}'")))?;
            return Ok(CallArg::Long(value));
        }
        if text.starts_with('"') {
            return Ok(CallArg::Str(
                parse_string(text).map_err(|message| self.bad(message))?,
            ));
        }
        let value: i32 = text
            .parse()
            .map_err(|_| self.bad(format!("bad call argument '{text}'")))?;
        Ok(CallArg::Int(value))
    }

    /// A call argument's stack type (`$locals` via declarations).
    fn call_arg_kind(&self, arg: &CallArg) -> std::result::Result<ValType, ParseError> {
        match arg {
            CallArg::Int(_) | CallArg::VarBit(..) => Ok(ValType::Int),
            CallArg::Long(_) => Ok(ValType::Long),
            CallArg::Str(_) => Ok(ValType::String),
            CallArg::Local(name) => self
                .args
                .iter()
                .chain(&self.locals)
                .find(|param| param.name == *name)
                .map(|param| param.ty)
                .ok_or_else(|| self.bad(format!("undeclared local '${name}'"))),
        }
    }

    /// Declared type of a `$`-less local/argument name, if declared.
    /// Declarations precede the body, so the tables are complete for every
    /// statement line this runs on.
    fn local_type(&self, name: &str) -> Option<ValType> {
        self.args
            .iter()
            .chain(&self.locals)
            .find(|param| param.name == name)
            .map(|param| param.ty)
    }

    /// `$name = <expr>;` — an expression statement. The target must be
    /// declared and the expression's type must match it; `~calls` in value
    /// position, bare computed values (no `=`), and unverified operators all
    /// fail loudly with the line. The `=` split is on the FIRST `=`
    /// (targets contain no `=`, so `$s = "a=b";` is safe); a `==` leaves a
    /// leading `=` the expression lexer rejects.
    fn parse_assign(
        &self,
        code: &str,
    ) -> std::result::Result<(String, crate::expr::Expr), ParseError> {
        let inner = code
            .strip_suffix(';')
            .ok_or_else(|| self.bad(format!("assignment misses its ';': '{code}'")))?;
        let (target_text, expr_text) = inner.split_once('=').ok_or_else(|| {
            self.bad(format!(
                "malformed assignment '{code}' (expected '$name = <expr>;')"
            ))
        })?;
        let name = parse_local_name(target_text.trim()).ok_or_else(|| {
            self.bad(format!(
                "bad assignment target '{}' (expected '$name')",
                target_text.trim()
            ))
        })?;
        let target_ty = self
            .local_type(&name)
            .ok_or_else(|| self.bad(format!("undeclared local '${name}'")))?;
        let expr_text = expr_text.trim();
        if expr_text.is_empty() {
            return Err(self.bad(format!("empty expression in assignment to '${name}'")));
        }
        let expr =
            crate::expr::parse_expr(expr_text, self.inames).map_err(|message| self.bad(message))?;
        let expr_ty = crate::expr::expr_type(
            &expr,
            &|local| self.local_type(local),
            self.symbols,
            self.configs,
        )
        .map_err(|message| self.bad(message))?;
        if expr_ty != target_ty {
            return Err(self.bad(format!(
                "cannot assign {} expression to {} '${name}'",
                expr_ty.keyword(),
                target_ty.keyword()
            )));
        }
        Ok((name, expr))
    }

    fn parse_instr(&self, code: &str) -> std::result::Result<SourceInstr, ParseError> {
        let (head, tail) = code.split_once('(').ok_or_else(|| {
            self.bad(format!(
                "malformed instruction '{code}' (expected 'command(operand);')"
            ))
        })?;
        let command = head.trim();
        check_ident(command, "command").map_err(|message| self.bad(message))?;
        let operand_text = tail
            .strip_suffix(");")
            .ok_or_else(|| self.bad(format!("instruction misses its ');': '{code}'")))?;
        let operand_text = operand_text.trim();
        if operand_text.is_empty() {
            return Err(self.bad(format!("'{command}' expects an operand")));
        }
        let operand = self.parse_operand(command, operand_text)?;
        // `jump` is the server-vocabulary sugar for the `branch` command.
        let command = if command == "jump" { "branch" } else { command };
        Ok(SourceInstr {
            command: command.to_string(),
            operand,
        })
    }

    fn parse_operand(
        &self,
        command: &str,
        text: &str,
    ) -> std::result::Result<SourceOperand, ParseError> {
        if command == "push_constant_int" {
            if let Some(component) = self.parse_component_operand(text)? {
                return Ok(component);
            }
            return Ok(SourceOperand::Int(self.parse_int(text)?));
        }
        if command == "push_constant_string" {
            return self.parse_const_string(text);
        }
        if INT_LOCAL_COMMANDS.contains(&command)
            || STRING_LOCAL_COMMANDS.contains(&command)
            || LONG_LOCAL_COMMANDS.contains(&command)
        {
            return Ok(SourceOperand::Local(self.parse_local_ref(text)?));
        }
        if command == "push_var" || command == "pop_var" {
            return self.parse_var(text);
        }
        if command == "push_varbit" || command == "pop_varbit" {
            return self.parse_varbit(text);
        }
        if is_branch_command(command) || command == "jump" {
            check_ident(text, "branch target").map_err(|message| self.bad(message))?;
            if text.starts_with('$') {
                return Err(self.bad("branch targets are labels, not $locals".to_string()));
            }
            return Ok(SourceOperand::Label(text.to_string()));
        }
        if command == "switch" {
            return Err(self.bad("switch takes a case block, not an operand".to_string()));
        }
        if command == "join_string" {
            return Ok(SourceOperand::Count(self.parse_int(text)?));
        }
        if command == "gosub_with_params" {
            return Ok(SourceOperand::Script(self.parse_int(text)?));
        }
        if ARRAY_COMMANDS.contains(&command) {
            return Ok(SourceOperand::Array(self.parse_int(text)?));
        }
        Ok(SourceOperand::Raw(self.parse_int(text)?))
    }

    fn parse_int(&self, text: &str) -> std::result::Result<i32, ParseError> {
        text.parse()
            .map_err(|_| self.bad(format!("bad int operand '{text}'")))
    }

    fn parse_const_string(&self, text: &str) -> std::result::Result<SourceOperand, ParseError> {
        // Symbolic component refs first: quoted strings never contain a bare
        // `Name/child` shape (the quote poisons the name check below), so the
        // string discipline below is unreachable for them and vice versa.
        if let Some(component) = self.parse_component_operand(text)? {
            return Ok(component);
        }
        if let Some(digits) = text.strip_suffix('L') {
            let value: i64 = digits
                .parse()
                .map_err(|_| self.bad(format!("bad long operand '{text}' (expected '<int>L')")))?;
            return Ok(SourceOperand::Long(value));
        }
        if text.parse::<i32>().is_ok() {
            return Ok(SourceOperand::Int(
                text.parse()
                    .map_err(|_| self.bad(format!("bad int operand '{text}'")))?,
            ));
        }
        Ok(SourceOperand::Str(
            parse_string(text).map_err(|message| self.bad(message))?,
        ))
    }

    /// A `Bank/7`-style component ref operand, or `None` when the text is not
    /// NAME-led symbolic shape. Only the shape gates here: numeric-led text
    /// (`1253/4`), quoted strings (`"a/b"`), and plain literals all fall
    /// through to the command's own literal parse (which fails loudly on its
    /// own for garbage). A NAME-led shape that fails to resolve is a loud
    /// line error — unknown interface, unknown child, or empty registry —
    /// never a silent number.
    fn parse_component_operand(
        &self,
        text: &str,
    ) -> std::result::Result<Option<SourceOperand>, ParseError> {
        let Some((iface_text, child_text)) = text.split_once('/') else {
            return Ok(None);
        };
        let (iface_text, child_text) = (iface_text.trim(), child_text.trim());
        if !is_valid_name(iface_text) {
            return Ok(None);
        }
        let Some((iface, child)) = self.inames.resolve_component(iface_text, child_text) else {
            if self.inames.resolve_iface(iface_text).is_none() {
                return Err(self.bad(format!("unknown interface '{iface_text}'")));
            }
            return Err(self.bad(format!("unknown component '{iface_text}/{child_text}'")));
        };
        Ok(Some(SourceOperand::Component { iface, child }))
    }

    fn parse_local_ref(&self, text: &str) -> std::result::Result<String, ParseError> {
        parse_local_name(text)
            .ok_or_else(|| self.bad(format!("bad local '{text}' (expected '$name')")))
    }

    fn parse_var(&self, text: &str) -> std::result::Result<SourceOperand, ParseError> {
        let mut parts = text.split(':');
        let domain_text = parts.next().unwrap_or("");
        let id_text = parts.next().ok_or_else(|| {
            self.bad(format!(
                "bad var '{text}' (expected 'domain:id[:transmog]')"
            ))
        })?;
        let domain = VarScope::from_label(domain_text)
            .map_err(|_| self.bad(format!("unknown var domain '{domain_text}'")))?;
        let id: u16 = id_text
            .parse()
            .map_err(|_| self.bad(format!("bad var id '{id_text}'")))?;
        let transmog = match parts.next() {
            None => false,
            Some("transmog") => true,
            Some(extra) => {
                return Err(self.bad(format!("bad var suffix '{extra}' (expected 'transmog')")));
            }
        };
        if parts.next().is_some() {
            return Err(self.bad(format!(
                "bad var '{text}' (expected 'domain:id[:transmog]')"
            )));
        }
        Ok(SourceOperand::Var(domain, id, transmog))
    }

    fn parse_varbit(&self, text: &str) -> std::result::Result<SourceOperand, ParseError> {
        let rest = text.strip_prefix("varbit:").ok_or_else(|| {
            self.bad(format!(
                "bad varbit '{text}' (expected 'varbit:id[:transmog]')"
            ))
        })?;
        let mut parts = rest.split(':');
        let id: u16 = parts
            .next()
            .unwrap_or("")
            .parse()
            .map_err(|_| self.bad(format!("bad varbit id in '{text}'")))?;
        let transmog = match parts.next() {
            None => false,
            Some("transmog") => true,
            Some(extra) => return Err(self.bad(format!("bad varbit suffix '{extra}'"))),
        };
        if parts.next().is_some() {
            return Err(self.bad(format!("bad varbit '{text}'")));
        }
        Ok(SourceOperand::VarBit(id, transmog))
    }

    fn check_param_fresh(&self, name: &str) -> std::result::Result<(), ParseError> {
        if self
            .args
            .iter()
            .chain(&self.locals)
            .any(|param| param.name == name)
        {
            return Err(self.bad(format!("duplicate parameter '${name}'")));
        }
        Ok(())
    }

    fn check_label_fresh(&self, label: &str) -> std::result::Result<(), ParseError> {
        if self.labels.iter().any(|(name, _)| name == label) {
            return Err(self.bad(format!("duplicate label '{label}'")));
        }
        Ok(())
    }

    fn finish(self) -> std::result::Result<SourceScript, ParseError> {
        if !self.header_done {
            return Err(ParseError {
                line: 1,
                message: "missing script header".to_string(),
            });
        }
        if self.switch_cases.is_some() {
            return Err(ParseError {
                line: self.line_no,
                message: "unclosed switch block".to_string(),
            });
        }
        Ok(SourceScript {
            name: self.name,
            args: self.args,
            locals: self.locals,
            body: self.body,
        })
    }
}

/// A local declaration line (`int $a, $b;`) → its type and names, or `None`
/// when the line is not a declaration.
fn parse_decl(code: &str) -> Option<std::result::Result<(ValType, Vec<String>), String>> {
    let (keyword, rest) = code.split_once(' ')?;
    if !matches!(keyword, "int" | "string" | "long") {
        return None;
    }
    Some(parse_decl_rest(keyword, rest))
}

fn parse_decl_rest(
    keyword: &str,
    rest: &str,
) -> std::result::Result<(ValType, Vec<String>), String> {
    let ty = ValType::parse_keyword(keyword).map_err(|error| error.to_string())?;
    let list = rest
        .strip_suffix(';')
        .ok_or_else(|| format!("declaration misses its ';': '{rest}'"))?;
    let mut names = Vec::new();
    for item in list.split(',') {
        let item = item.trim();
        names.push(
            parse_local_name(item)
                .ok_or_else(|| format!("bad local name '{item}' (expected '$name')"))?,
        );
    }
    if names.is_empty() {
        return Err("empty declaration".to_string());
    }
    Ok((ty, names))
}

fn parse_local_name(text: &str) -> Option<String> {
    let name = text.strip_prefix('$')?;
    check_ident(name, "local").ok()?;
    Some(name.to_string())
}

fn check_ident(text: &str, what: &str) -> std::result::Result<(), String> {
    if is_valid_name(text) {
        Ok(())
    } else {
        Err(format!("bad {what} '{text}'"))
    }
}

/// Split call arguments on top-level commas. String literals may contain
/// commas (game text does constantly), so the scan tracks quotes and escapes;
/// `$locals` and numbers never contain commas.
fn split_call_args(text: &str) -> std::result::Result<Vec<&str>, String> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0_usize;
    let mut in_string = false;
    let mut position = 0_usize;
    while position < bytes.len() {
        let byte = bytes[position];
        if in_string {
            if byte == b'\\' {
                position += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            position += 1;
            continue;
        }
        if byte == b',' {
            parts.push(text[start..position].trim());
            start = position + 1;
        }
        position += 1;
    }
    if in_string {
        return Err(format!("unterminated string in call arguments '{text}'"));
    }
    parts.push(text[start..].trim());
    Ok(parts)
}

/// Parse a quoted string literal with `\\ \" \n \r \t` escapes.
fn parse_string(text: &str) -> std::result::Result<String, String> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| format!("bad string {text:?} (expected double quotes)"))?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => return Err(format!("bad escape '\\{other}' in {text:?}")),
            None => return Err(format!("dangling escape in {text:?}")),
        }
    }
    Ok(out)
}

/// Cut a `//` comment, ignoring `//` inside string literals.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut position = 0;
    while position < bytes.len() {
        let byte = bytes[position];
        if in_string {
            if byte == b'\\' {
                position += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            position += 1;
            continue;
        }
        if byte == b'/' && bytes.get(position + 1) == Some(&b'/') {
            return &line[..position];
        }
        position += 1;
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigTypes;
    use crate::opcode::OpcodeBook;
    use crate::source::{assemble_source, format_source};
    use crate::symbols::SymbolRegistry;

    fn symbols() -> SymbolRegistry {
        SymbolRegistry::empty()
    }

    fn inames() -> crate::inames::InterfaceRegistry {
        crate::inames::InterfaceRegistry::empty()
    }

    /// Registry naming interface 1253 `bank` (child 4 `main`) over a roster
    /// holding children 4 and 7.
    fn comp_names() -> crate::inames::InterfaceRegistry {
        let (ifaces, children) =
            crate::inames::parse_inames_txt("interface 1253 bank\ncomponent 1253 4 main\n")
                .unwrap();
        let roster = std::collections::BTreeMap::from([(1253, vec![4, 7])]);
        crate::inames::InterfaceRegistry::build(&ifaces, &children, &roster).unwrap()
    }

    fn configs() -> ConfigTypes {
        ConfigTypes::empty()
    }

    const EXAMPLE: &str = "\
[clientscript,example](int $param0) // trailing comment
int $temp0;
string $temp1; // comment

    push_constant_int(41);
    push_int_local($param0);
    add(0);
    pop_int_local($temp0);
    branch_if_false(end);
    push_constant_string(\"done\");
    pop_string_local($temp1);
end:
    push_int_local($temp0);
    switch {
        case 0 -> end;
        case -1 -> start;
    }
start:
    return(0);
";

    #[test]
    fn example_parses_to_expected_model() {
        let script = parse_source(EXAMPLE, &symbols(), &configs(), &inames()).unwrap();
        assert_eq!(script.name.as_deref(), Some("clientscript,example"));
        assert_eq!(script.args.len(), 1);
        assert_eq!(script.locals.len(), 2);
        assert_eq!(script.body.len(), 12);
        assert!(matches!(
            script.body[7],
            SourceStmt::Label(ref name) if name == "end"
        ));
    }

    #[test]
    fn jump_sugar_assembles_as_branch() {
        let book = OpcodeBook::embedded().unwrap();
        let bytes = assemble_source(
            "[p]()\n\n    jump(done);\ndone:\n    return(0);\n",
            &book,
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap();
        let decoded = crate::script::decode_script(&bytes, &book).unwrap();
        assert_eq!(decoded.code[0].command, "branch");
        assert!(matches!(
            decoded.code[0].operand,
            crate::script::Operand::Branch(1)
        ));
    }

    #[test]
    fn parse_format_is_stable() {
        let first = parse_source(EXAMPLE, &symbols(), &configs(), &inames()).unwrap();
        let second = parse_source(
            &format_source(&first, &symbols(), &inames()).unwrap(),
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn empty_switch_roundtrips() {
        let book = OpcodeBook::embedded().unwrap();
        let text = "[]()\n\n    switch {\n    }\n    return(0);\n";
        let bytes = assemble_source(text, &book, &symbols(), &configs(), &inames()).unwrap();
        let decoded = crate::script::decode_script(&bytes, &book).unwrap();
        assert!(matches!(
            decoded.code[0].operand,
            crate::script::Operand::Switch(ref cases) if cases.is_empty()
        ));
    }

    #[test]
    fn comment_inside_string_survives() {
        let script = parse_source(
            "[p]()\n\n    push_constant_string(\"a//b\");\n    return(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap();
        assert!(matches!(
            &script.body[0],
            SourceStmt::Instr(instr)
            if matches!(&instr.operand, SourceOperand::Str(text) if text == "a//b")
        ));
    }

    #[test]
    fn named_calls_resolve_and_render() {
        use std::collections::BTreeMap;
        let known = BTreeMap::from([(5690, crate::script::Counts::default())]);
        let registry =
            SymbolRegistry::build(vec![(5690, "bank_build_init".to_string())], &known).unwrap();
        let book = OpcodeBook::embedded().unwrap();

        // `~name();` parses to a Call for the same callee.
        let named = parse_source(
            "[p]()\n\n    ~bank_build_init();\n    return(0);\n",
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap();
        assert!(matches!(
            &named.body[0],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Call(name, args) if name == "bank_build_init" && args.is_empty())
        ));
        let numeric = parse_source(
            "[p]()\n\n    gosub_with_params(5690);\n    return(0);\n",
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap();

        // Formatting prefers the name; assembling it reproduces numeric bytes.
        let text = format_source(&numeric, &registry, &inames()).unwrap();
        assert!(text.contains("    ~bank_build_init();\n"));
        let from_name = assemble_source(&text, &book, &registry, &configs(), &inames()).unwrap();
        let from_number = assemble_source(
            "[p]()\n\n    gosub_with_params(5690);\n    return(0);\n",
            &book,
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap();
        assert_eq!(from_name, from_number);

        // Unknown names fail with a line.
        let error = parse_source(
            "[p]()\n\n    ~ghost();\n    return(0);\n",
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
    }

    #[test]
    fn calls_validate_arguments_positionally_free() {
        use std::collections::BTreeMap;
        // Callee takes (2i, 1o, 0l).
        let known = BTreeMap::from([(
            100,
            crate::script::Counts {
                int: 2,
                obj: 1,
                long: 0,
            },
        )]);
        let registry =
            SymbolRegistry::build(vec![(100, "takes_three".to_string())], &known).unwrap();
        let book = OpcodeBook::embedded().unwrap();

        // Interleaved order is fine (multiset rule); $locals resolve by type.
        let text =
            "[p]()\nint $a;\nstring $b;\n\n    ~takes_three(\"x\", $a, $b, 41);\n    return(0);\n";
        let error = parse_source(text, &registry, &configs(), &inames()).unwrap_err();
        assert_eq!(error.line, 5);
        // Four args for a three-arg callee.
        let text = "[p]()\nint $a;\nstring $b;\n\n    ~takes_three($b, $a, 41);\n    return(0);\n";
        let script = parse_source(text, &registry, &configs(), &inames()).unwrap();
        let bytes = assemble_source(
            &format_source(&script, &registry, &inames()).unwrap(),
            &book,
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap();
        let decoded = crate::script::decode_script(&bytes, &book).unwrap();
        // Lowered in listed order: string-local push, int-local push,
        // const-string-int push, gosub, return.
        assert_eq!(decoded.code.len(), 5);
        assert_eq!(decoded.code[0].command, "push_string_local");
        assert_eq!(decoded.code[1].command, "push_int_local");
        assert_eq!(decoded.code[2].command, "push_constant_string");
        assert!(matches!(
            decoded.code[2].operand,
            crate::script::Operand::Int(41)
        ));
        assert_eq!(decoded.code[3].command, "gosub_with_params");

        // Wrong multiset (two strings) fails with a line.
        let error = parse_source(
            "[p]()\nstring $b;\n\n    ~takes_three(\"x\", \"y\", 41);\n    return(0);\n",
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 4);

        // Undeclared $local fails with a line.
        let error = parse_source(
            "[p]()\n\n    ~takes_three(\"x\", $ghost, 41);\n    return(0);\n",
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
    }

    #[test]
    fn call_arguments_survive_hostile_game_text() {
        use std::collections::BTreeMap;
        let known = BTreeMap::from([(
            100,
            crate::script::Counts {
                int: 0,
                obj: 2,
                long: 0,
            },
        )]);
        let registry =
            SymbolRegistry::build(vec![(100, "shows_text".to_string())], &known).unwrap();
        let book = OpcodeBook::embedded().unwrap();

        // Commas, escaped quotes, and comment markers inside strings.
        let text =
            "[p]()\n\n    ~shows_text(\"a, b\", \"say \\\"hi\\\" // ok\");\n    return(0);\n";
        let script = parse_source(text, &registry, &configs(), &inames()).unwrap();
        let bytes = assemble_source(
            &format_source(&script, &registry, &inames()).unwrap(),
            &book,
            &registry,
            &configs(),
            &inames(),
        )
        .unwrap();
        let decoded = crate::script::decode_script(&bytes, &book).unwrap();
        assert_eq!(decoded.code.len(), 4);
        assert!(matches!(
            &decoded.code[0].operand,
            crate::script::Operand::Str(first) if first == "a, b"
        ));
        assert!(matches!(
            &decoded.code[1].operand,
            crate::script::Operand::Str(second) if second == "say \"hi\" // ok"
        ));
    }

    #[test]
    fn failures_name_their_lines() {
        // No header.
        let error = parse_source("    add(0);\n", &symbols(), &configs(), &inames()).unwrap_err();
        assert_eq!(error.line, 1);
        // Duplicate label on line 5.
        let error = parse_source(
            "[p]()\n\na:\n    return(0);\na:\n    return(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 5);
        // Unclosed switch.
        let error = parse_source(
            "[p]()\n\n    switch {\n        case 0 -> a;\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
        // Bad literal.
        let error = parse_source(
            "[p]()\n\n    push_constant_int(x);\n    return(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
        // Empty operand.
        let error =
            parse_source("[p]()\n\n    add();\n", &symbols(), &configs(), &inames()).unwrap_err();
        assert_eq!(error.line, 3);
        // Inline switch.
        let error = parse_source(
            "[p]()\n\n    switch(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
    }

    #[test]
    fn assignments_parse_and_name_their_lines() {
        // A well-typed assignment parses to the Assign model.
        let script = parse_source(
            "[p]()\nint $a, $y;\n\n    $y = $a + 41;\n    return(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap();
        assert!(matches!(
            &script.body[0],
            SourceStmt::Assign { target, expr }
                if target == "y" && crate::expr::format_expr(expr, &inames()) == "$a + 41"
        ));
        // Undeclared target, type mismatch, bare `$` line, and missing `;`
        // each fail with their own line.
        let error = parse_source(
            "[p]()\nint $y;\n\n    $ghost = 1;\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
        let error = parse_source(
            "[p]()\nint $y;\n\n    $y = \"s\";\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
        assert!(error.message.contains("cannot assign string"));
        let error = parse_source(
            "[p]()\nint $a;\n\n    $a + 1;\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
        let error = parse_source(
            "[p]()\nint $y;\n\n    $y = 1\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
    }

    #[test]
    fn component_operands_assemble_like_their_packed_ints() {
        use crate::source::assemble_source;
        let book = OpcodeBook::embedded().unwrap();
        let names = comp_names();
        // Both int-literal slots accept the symbolic shape, lowering to the
        // identical bytes as the numeric spelling (the equivalence-test
        // style: same command, same operand value, both directions).
        for command in ["push_constant_string", "push_constant_int"] {
            let symbolic = format!("[p]()\n\n    {command}(bank/main);\n    return(0);\n");
            let numeric = format!("[p]()\n\n    {command}(82116612);\n    return(0);\n");
            assert_eq!(
                assemble_source(&symbolic, &book, &symbols(), &configs(), &names).unwrap(),
                assemble_source(&numeric, &book, &symbols(), &configs(), &names).unwrap(),
            );
            let symbolic = format!("[p]()\n\n    {command}(bank/7);\n    return(0);\n");
            let numeric = format!("[p]()\n\n    {command}(82116615);\n    return(0);\n");
            assert_eq!(
                assemble_source(&symbolic, &book, &symbols(), &configs(), &names).unwrap(),
                assemble_source(&numeric, &book, &symbols(), &configs(), &names).unwrap(),
            );
        }
        // Expression positions join in: `$y = bank/main;` assembles like the
        // flat push/pop of the packed int.
        let symbolic = "[p]()\nint $y;\n\n    $y = bank/main;\n    return(0);\n";
        let flat = "[p]()\nint $y;\n\n    push_constant_string(82116612);\n    pop_int_local($y);\n    return(0);\n";
        assert_eq!(
            assemble_source(symbolic, &book, &symbols(), &configs(), &names).unwrap(),
            assemble_source(flat, &book, &symbols(), &configs(), &names).unwrap(),
        );
    }

    #[test]
    fn component_spellings_roundtrip_both_directions() {
        let names = comp_names();
        // Symbolic parses symbolic again; numeric stays numeric (hand-written
        // numbers never auto-symbolize, even when the registry knows them).
        for text in [
            "[p]()\n\n    push_constant_string(bank/main);\n    return(0);\n",
            "[p]()\n\n    push_constant_int(bank/7);\n    return(0);\n",
            "[p]()\nint $y;\n\n    $y = bank/main;\n    return(0);\n",
            // Components nest as atomic int leaves in larger trees.
            "[p]()\nint $a, $y;\n\n    $y = $a + bank/7 * 2;\n    return(0);\n",
        ] {
            let first = parse_source(text, &symbols(), &configs(), &names).unwrap();
            assert_eq!(format_source(&first, &symbols(), &names).unwrap(), text);
        }
        // Without names the same pair degrades to its packed decimal, which
        // re-parses as the identical int literal (never as a component).
        let symbolic = parse_source(
            "[p]()\nint $y;\n\n    $y = bank/main;\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap();
        let degraded = format_source(&symbolic, &symbols(), &inames()).unwrap();
        assert!(
            degraded.contains("    $y = 82116612;\n"),
            "unexpected text:\n{degraded}"
        );
        let reparsed = parse_source(&degraded, &symbols(), &configs(), &inames()).unwrap();
        assert!(matches!(
            &reparsed.body[0],
            SourceStmt::Assign { expr, .. } if *expr == crate::expr::Expr::LitInt(82_116_612)
        ));
        for text in [
            "[p]()\n\n    push_constant_string(82116612);\n    return(0);\n",
            "[p]()\nint $y;\n\n    $y = 82116612;\n    return(0);\n",
        ] {
            let first = parse_source(text, &symbols(), &configs(), &names).unwrap();
            assert_eq!(format_source(&first, &symbols(), &names).unwrap(), text);
        }
    }

    #[test]
    fn division_still_wins_in_assignments() {
        let names = comp_names();
        // `1253/4` is arithmetic even with the registry present.
        let script = parse_source(
            "[p]()\nint $y;\n\n    $y = 1253/4;\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap();
        assert!(matches!(
            &script.body[0],
            SourceStmt::Assign { target, expr }
                if target == "y"
                    && *expr
                        == crate::expr::Expr::Binary(
                            crate::expr::BinaryOp::Divide,
                            Box::new(crate::expr::Expr::LitInt(1253)),
                            Box::new(crate::expr::Expr::LitInt(4)),
                        )
        ));
        let text = format_source(&script, &symbols(), &names).unwrap();
        assert!(
            text.contains("    $y = 1253 / 4;\n"),
            "unexpected text:\n{text}"
        );
        // A numeric head never starts a component, even naming a real child.
        let error = parse_source(
            "[p]()\nint $y;\n\n    $y = 1253/main;\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
    }

    #[test]
    fn component_failures_name_their_lines() {
        let names = comp_names();
        // Unknown interface, unknown child, roster-absent numeric child.
        let error = parse_source(
            "[p]()\n\n    push_constant_string(ghost/1);\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
        assert!(error.message.contains("unknown interface 'ghost'"));
        let error = parse_source(
            "[p]()\n\n    push_constant_int(bank/ghost);\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
        assert!(error.message.contains("unknown component 'bank/ghost'"));
        let error = parse_source(
            "[p]()\nint $y;\n\n    $y = bank/9;\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
        assert!(error.message.contains("unknown component 'bank/9'"));
        // Malformed children fail, never divide or guess.
        for child in ["", "$a", "-5", "7L"] {
            let text = format!("[p]()\nint $a, $y;\n\n    $y = bank/{child};\n    return(0);\n");
            let error = parse_source(&text, &symbols(), &configs(), &names)
                .expect_err(&format!("accepted bank/{child}"));
            assert_eq!(error.line, 4, "bank/{child}");
        }
        // An empty registry rejects every symbolic ref; numeric text parses on.
        let error = parse_source(
            "[p]()\n\n    push_constant_string(bank/7);\n    return(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
        parse_source(
            "[p]()\n\n    push_constant_string(82116615);\n    return(0);\n",
            &symbols(),
            &configs(),
            &inames(),
        )
        .unwrap();
        // Numeric/numeric stays a bad int in statement operands (division
        // lives in expressions only), never a component.
        let error = parse_source(
            "[p]()\n\n    push_constant_string(1253/4);\n    return(0);\n",
            &symbols(),
            &configs(),
            &names,
        )
        .unwrap_err();
        assert_eq!(error.line, 3);
    }
}
