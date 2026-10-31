//! `calculate` 工具（ReadOnly）：纯 Rust 表达式求值器，
//! 防模型心算出错。
//!
//! 选型决议（两候选定案）：首选评估过第三方库 `evalexpr`
//! （纯 Rust / MIT / 空上下文下无函数调用面，评估通过），但其错误类型
//! **不携带 token 位置**，而本工具要求除零/语法错误以 E_INVALID_ARG
//! 带出错位置返回，无法满足；故走**手写递归下降解析器**（约 300 行零依赖）——
//! 错误位置由词法/解析层原生携带，且求值语义完全可控。定案标准逐条核对：
//! 无副作用（纯求值）、无 IO、无任意函数调用（标识符仅 pi/e，函数调用
//! 形态显式报错）、浮点结果与 Python/bc 对照精确（f64 + 最短往返格式化，
//! 见 [`format_value`]）。
//!
//! 语义口径（description 同步说明）：
//! - 运算：`+ - * / % ^`、一元 `+ -`、括号、比较 `< <= > >= == !=`；
//! - 整数字面量走 i64（checked 溢出报错并提示改浮点），含小数点/指数走
//!   f64；除法永远浮点（Python `/` 语义，`7/2 = 3.5`）；`%` 为 Rust 截断
//!   取余（与 Python 负数取余结果不同，负数取余建议用 Python 语义校准的
//!   表达式改写）；`^` 为幂（右结合，`2^3^2 = 512`）；
//! - 除零（含浮点除零）、整数溢出、非有限结果（inf/NaN）均 E_INVALID_ARG；
//! - 无赋值、无多语句、无函数调用（`=` 单独出现按「不支持赋值」报错）。

use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec};
use crate::util::errors::ToolError;

/// 求值结果值：整数、浮点或布尔（比较运算产物）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl Value {
    /// 数值视图：参与算术时整数升浮点（混合运算统一 f64）。
    fn as_f64(self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            Value::Bool(_) => None,
        }
    }
}

/// 求值/解析错误：出错字节位置 + 人话说明（转 E_INVALID_ARG 时换算成
/// 1-based 字符位置，错误信息带出错位置）。
#[derive(Debug)]
struct CalcError {
    /// 出错位置的字节偏移。
    pos: usize,
    message: String,
}

impl CalcError {
    fn new(pos: usize, message: impl Into<String>) -> Self {
        Self {
            pos,
            message: message.into(),
        }
    }
}

/// 词元类型。
#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    /// 数字字面量（整数或浮点，词法期已定通道）。
    Num(Value),
    /// 标识符（仅接受 pi/e；其余在词法期报错）。
    Ident(&'static str),
    /// 运算符（两字符优先匹配：== != <= >=）。
    Op(&'static str),
    /// 左括号。
    LParen,
    /// 右括号。
    RParen,
}

/// 带位置的词元。
#[derive(Debug, Clone, PartialEq)]
struct Token {
    kind: TokenKind,
    pos: usize,
}

/// 词法分析：把表达式切成词元序列；无法识别的字符/标识符在此报错。
fn tokenize(src: &str) -> Result<Vec<Token>, CalcError> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'(' => {
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    pos: i,
                });
                i += 1;
            }
            b')' => {
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    pos: i,
                });
                i += 1;
            }
            b'0'..=b'9' | b'.' => {
                let start = i;
                let mut has_digit = false;
                let mut has_dot = false;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    if bytes[i] == b'.' {
                        if has_dot {
                            return Err(CalcError::new(i, "数字里出现第二个小数点"));
                        }
                        has_dot = true;
                    } else {
                        has_digit = true;
                    }
                    i += 1;
                }
                // 科学计数法：e/E + 可选正负号 + 指数数字。
                let mut has_exp = false;
                if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                    let mut j = i + 1;
                    if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j].is_ascii_digit() {
                        has_exp = true;
                        i = j;
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                if !has_digit {
                    return Err(CalcError::new(start, "孤立的点不是合法数字（如 0.5）"));
                }
                let text = &src[start..i];
                if has_dot || has_exp {
                    let f = text
                        .parse::<f64>()
                        .map_err(|_| CalcError::new(start, format!("非法数字字面量：{text}")))?;
                    tokens.push(Token {
                        kind: TokenKind::Num(Value::Float(f)),
                        pos: start,
                    });
                } else {
                    let n = text.parse::<i64>().map_err(|_| {
                        CalcError::new(
                            start,
                            format!("整数 {text} 超出 64 位范围：请用浮点表示（如 {text}.0 或科学计数法）"),
                        )
                    })?;
                    tokens.push(Token {
                        kind: TokenKind::Num(Value::Int(n)),
                        pos: start,
                    });
                }
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let name = &src[start..i];
                // 常量白名单：pi / e（数学常量）。函数调用形态（标识符后跟
                // 括号）在解析层给出更贴切的报错；其他名称一律拒绝。
                match name {
                    "pi" | "PI" => tokens.push(Token {
                        kind: TokenKind::Ident("pi"),
                        pos: start,
                    }),
                    "e" | "E" => tokens.push(Token {
                        kind: TokenKind::Ident("e"),
                        pos: start,
                    }),
                    // 布尔字面量（比较结果的再比较需要：如 (1>2)==false）。
                    "true" => tokens.push(Token {
                        kind: TokenKind::Num(Value::Bool(true)),
                        pos: start,
                    }),
                    "false" => tokens.push(Token {
                        kind: TokenKind::Num(Value::Bool(false)),
                        pos: start,
                    }),
                    other => {
                        return Err(CalcError::new(
                            start,
                            format!("不支持的名称 {other:?}：求值器只有 pi/e 两个常量、无函数调用"),
                        ));
                    }
                }
            }
            _ => {
                // 运算符：两字符优先（== != <= >=），单字符其次。
                let two = if i + 1 < bytes.len() {
                    Some(&src[i..i + 2])
                } else {
                    None
                };
                let op2 = match two {
                    Some("==") => Some("=="),
                    Some("!=") => Some("!="),
                    Some("<=") => Some("<="),
                    Some(">=") => Some(">="),
                    _ => None,
                };
                if let Some(op) = op2 {
                    tokens.push(Token {
                        kind: TokenKind::Op(op),
                        pos: i,
                    });
                    i += 2;
                    continue;
                }
                let op1 = match b {
                    b'+' => "+",
                    b'-' => "-",
                    b'*' => "*",
                    b'/' => "/",
                    b'%' => "%",
                    b'^' => "^",
                    b'<' => "<",
                    b'>' => ">",
                    _ => "",
                };
                if !op1.is_empty() {
                    tokens.push(Token {
                        kind: TokenKind::Op(op1),
                        pos: i,
                    });
                    i += 1;
                    continue;
                }
                if b == b'=' {
                    return Err(CalcError::new(
                        i,
                        "不支持赋值：calculate 只做求值（比较相等请写 ==）",
                    ));
                }
                let ch = src[i..].chars().next().expect("非空字节序必有字符");
                return Err(CalcError::new(i, format!("无法识别的字符 {ch:?}")));
            }
        }
    }
    Ok(tokens)
}

/// 递归下降解析 + 求值（边解析边算，无中间 AST）。
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    /// 取当前词元位置（越界时用最后位置 + 1，报「表达式意外结束」）。
    fn here(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|t| t.pos)
            .unwrap_or_else(|| self.tokens.last().map(|t| t.pos + 1).unwrap_or(0))
    }

    fn eat_op(&mut self, ops: &[&str]) -> Option<(&'static str, usize)> {
        if let Some(Token {
            kind: TokenKind::Op(op),
            pos,
        }) = self.tokens.get(self.pos)
            && ops.contains(op)
        {
            let found = (*op, *pos);
            self.pos += 1;
            return Some(found);
        }
        None
    }

    fn expect_rparen(&mut self, lpos: usize) -> Result<(), CalcError> {
        match self.tokens.get(self.pos) {
            Some(Token {
                kind: TokenKind::RParen,
                ..
            }) => {
                self.pos += 1;
                Ok(())
            }
            Some(Token { pos, .. }) => Err(CalcError::new(*pos, "此处应为括号 )")),
            None => Err(CalcError::new(lpos, "表达式意外结束：括号 ( 未闭合")),
        }
    }

    /// 最高层：比较（同层从左到右，不允许链式大小比较——布尔不参与数值比较）。
    fn comparison(&mut self) -> Result<Value, CalcError> {
        let mut left = self.additive()?;
        while let Some((op, op_pos)) = self.eat_op(&["<", "<=", ">", ">=", "==", "!="]) {
            let right = self.additive()?;
            left = compare(op, op_pos, left, right)?;
        }
        Ok(left)
    }

    /// 加减层。
    fn additive(&mut self) -> Result<Value, CalcError> {
        let mut left = self.multiplicative()?;
        loop {
            let Some((op, op_pos)) = self.eat_op(&["+", "-"]) else {
                return Ok(left);
            };
            let right = self.multiplicative()?;
            left = arithmetic(op, op_pos, left, right)?;
        }
    }

    /// 乘除取余层。
    fn multiplicative(&mut self) -> Result<Value, CalcError> {
        let mut left = self.unary()?;
        loop {
            let Some((op, op_pos)) = self.eat_op(&["*", "/", "%"]) else {
                return Ok(left);
            };
            let right = self.unary()?;
            left = arithmetic(op, op_pos, left, right)?;
        }
    }

    /// 一元正负号（可叠：`--3 = 3`）。
    fn unary(&mut self) -> Result<Value, CalcError> {
        if let Some((op, op_pos)) = self.eat_op(&["+", "-"]) {
            let v = self.unary()?;
            return match (op, v) {
                ("+", Value::Bool(_)) | ("-", Value::Bool(_)) => {
                    Err(CalcError::new(op_pos, "布尔值不能参与算术运算"))
                }
                ("+", v) => Ok(v),
                ("-", Value::Int(i)) => i
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| CalcError::new(op_pos, "整数取负溢出：请改用浮点表示")),
                ("-", Value::Float(f)) => Ok(Value::Float(-f)),
                _ => unreachable!("上方已拦布尔"),
            };
        }
        self.power()
    }

    /// 幂层（右结合且指数可带一元符号：`2^-3`、`2^3^2 = 2^(3^2)`）。
    fn power(&mut self) -> Result<Value, CalcError> {
        let base = self.primary()?;
        if let Some((_, op_pos)) = self.eat_op(&["^"]) {
            let exponent = self.unary()?;
            return pow(op_pos, base, exponent);
        }
        Ok(base)
    }

    /// 最内层：数字 / 常量 / 括号。标识符后跟括号 = 函数调用，显式拒绝。
    fn primary(&mut self) -> Result<Value, CalcError> {
        let Some(token) = self.tokens.get(self.pos).cloned() else {
            return Err(CalcError::new(self.here(), "表达式意外结束：缺少操作数"));
        };
        self.pos += 1;
        match token.kind {
            TokenKind::Num(v) => Ok(v),
            TokenKind::Ident("pi") => Ok(Value::Float(std::f64::consts::PI)),
            TokenKind::Ident("e") => Ok(Value::Float(std::f64::consts::E)),
            TokenKind::Ident(_) => unreachable!("词法层只放行 pi/e"),
            TokenKind::Op(op) => Err(CalcError::new(
                token.pos,
                format!("此处应为数值，却是运算符 {op:?}"),
            )),
            TokenKind::LParen => {
                // 括号后紧跟下一个运算符层表达式；形如 `f(x)` 的调用已在
                // 标识符层被拦，此处括号只能是分组。
                let v = self.comparison()?;
                self.expect_rparen(token.pos)?;
                Ok(v)
            }
            TokenKind::RParen => Err(CalcError::new(token.pos, "意外的右括号 )")),
        }
    }
}

/// 混合算术：整数通道 checked，浮点参与即升 f64；除法永远浮点。
fn arithmetic(op: &str, op_pos: usize, left: Value, right: Value) -> Result<Value, CalcError> {
    // 除零统一在入口判定（整除零与浮点除零都直接报错，不让 inf/NaN 溜进结果）。
    if matches!(op, "/" | "%") {
        let zero = match right {
            Value::Int(0) => true,
            Value::Float(f) => f == 0.0,
            _ => false,
        };
        if zero {
            return Err(CalcError::new(op_pos, "除零：除数为 0"));
        }
    }
    if let (Value::Int(a), Value::Int(b)) = (left, right) {
        // 纯整数通道（除法除外：按 Python `/` 语义浮点除）。
        return match op {
            "+" => a.checked_add(b).map(Value::Int).ok_or_else(|| {
                CalcError::new(op_pos, "整数加法溢出：请改用浮点表示（数字后加 .0）")
            }),
            "-" => a.checked_sub(b).map(Value::Int).ok_or_else(|| {
                CalcError::new(op_pos, "整数减法溢出：请改用浮点表示（数字后加 .0）")
            }),
            "*" => a.checked_mul(b).map(Value::Int).ok_or_else(|| {
                CalcError::new(op_pos, "整数乘法溢出：请改用浮点表示（数字后加 .0）")
            }),
            "/" => Ok(Value::Float(a as f64 / b as f64)),
            "%" => a.checked_rem(b).map(Value::Int).ok_or_else(|| {
                CalcError::new(op_pos, "整数取余溢出：请改用浮点表示（数字后加 .0）")
            }),
            _ => unreachable!("arithmetic 只接收 + - * / %"),
        };
    }
    let (a, b) = match (left.as_f64(), right.as_f64()) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(CalcError::new(
                op_pos,
                "布尔值不能参与算术运算（比较请用 == / !=）",
            ));
        }
    };
    let r = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        "%" => a % b,
        _ => unreachable!("arithmetic 只接收 + - * / %"),
    };
    Ok(Value::Float(r))
}

/// 幂运算：整数底数与非负整数指数走 i64 checked_pow；其余 f64 powf。
fn pow(op_pos: usize, base: Value, exponent: Value) -> Result<Value, CalcError> {
    if let (Value::Int(b), Value::Int(e)) = (base, exponent)
        && e >= 0
    {
        let e = u32::try_from(e)
            .map_err(|_| CalcError::new(op_pos, "指数过大：请改用浮点表示（数字后加 .0）"))?;
        return b
            .checked_pow(e)
            .map(Value::Int)
            .ok_or_else(|| CalcError::new(op_pos, "整数幂溢出：请改用浮点表示（数字后加 .0）"));
    }
    let (b, e) = match (base.as_f64(), exponent.as_f64()) {
        (Some(b), Some(e)) => (b, e),
        _ => {
            return Err(CalcError::new(op_pos, "布尔值不能参与幂运算"));
        }
    };
    Ok(Value::Float(b.powf(e)))
}

/// 比较：布尔只允许 == / !=；数值混合类型升 f64 比较。
fn compare(op: &str, op_pos: usize, left: Value, right: Value) -> Result<Value, CalcError> {
    if matches!(left, Value::Bool(_)) || matches!(right, Value::Bool(_)) {
        if !matches!(op, "==" | "!=") {
            return Err(CalcError::new(
                op_pos,
                "布尔值只能做 == / != 比较（不支持链式大小比较）",
            ));
        }
        let result = match (left, right) {
            (Value::Bool(a), Value::Bool(b)) => {
                if op == "==" {
                    a == b
                } else {
                    a != b
                }
            }
            (a, b) => {
                if op == "==" {
                    a == b
                } else {
                    a != b
                }
            }
        };
        return Ok(Value::Bool(result));
    }
    let (a, b) = match (left.as_f64(), right.as_f64()) {
        (Some(a), Some(b)) => (a, b),
        _ => unreachable!("非布尔值必有 f64 视图"),
    };
    // 浮点相等比较按直接相等（与 Python == 一致）。
    let result = match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        "==" => a == b,
        "!=" => a != b,
        _ => unreachable!("compare 只接收六种比较符"),
    };
    Ok(Value::Bool(result))
}

/// 格式化结果（保真显示，防浮点截断误导）：整数直书、布尔直书、浮点用
/// Rust 最短往返表示（`0.1 + 0.2` → `0.30000000000000004`，与 Python repr
/// / bc 对照一致，不静默四舍五入）。
fn format_value(v: &Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Float(f) => {
            if f.is_finite() {
                format!("{f}")
            } else {
                unreachable!("非有限结果在求值出口已拦截")
            }
        }
    }
}

/// 出错字节偏移 → 1-based 字符位置（按 UTF-8 字符计，提示面向模型/人）。
fn char_position(src: &str, pos: usize) -> usize {
    src.get(..pos.min(src.len()))
        .map(|s| s.chars().count())
        .unwrap_or(0)
        + 1
}

/// 工具执行入口（与其他工具统一的双参签名，dispatch 面一致；calculate 不
/// 依赖工作区，ctx 不使用）——实际实现转调 [`eval_tool`]。
pub fn run(_ctx: &crate::tools::ToolsContext, args: &CalculateArgs) -> ToolOutcome {
    eval_tool(args)
}

/// 执行 calculate：词法 → 解析求值 → 有限性检查 → 结果三态
/// （result 数值 / formatted 保真字符串 / expression 原文）。
fn eval_tool(args: &CalculateArgs) -> ToolOutcome {
    let expression = args.expression.trim();
    let evaluated = evaluate(expression);
    match evaluated {
        Ok(value) => {
            // 非有限结果兜底（浮点 powf 等路径仍可能产出 inf/NaN）。
            if let Value::Float(f) = value
                && !f.is_finite()
            {
                return ToolError::invalid_arg(format!(
                    "表达式 {expression} 的结果不是有限数（inf/NaN）：请检查除零与超大指数"
                ))
                .into();
            }
            let formatted = format_value(&value);
            let result = match value {
                Value::Int(i) => json!(i),
                Value::Float(f) => json!(f),
                Value::Bool(b) => json!(b),
            };
            ToolOutcome::success(
                json!({
                    "expression": expression,
                    "result": result,
                    "formatted": formatted,
                }),
                format!("{expression} = {formatted}"),
            )
        }
        Err(e) => {
            let message = format!(
                "第 {} 个字符附近：{}",
                char_position(expression, e.pos),
                e.message
            );
            ToolOutcome::from(ToolError::invalid_arg(message))
        }
    }
}

/// 独立的求值入口（测试与工具共用）：词法 + 解析 + 残余词元检查。
fn evaluate(expression: &str) -> Result<Value, CalcError> {
    if expression.is_empty() {
        return Err(CalcError::new(0, "表达式为空"));
    }
    let tokens = tokenize(expression)?;
    if tokens.is_empty() {
        return Err(CalcError::new(0, "表达式为空"));
    }
    let mut parser = Parser::new(tokens);
    let value = parser.comparison()?;
    // 全部词元必须消费完（如 "1 2" 这类拼接输入在尾部报错并给位置）。
    if parser.pos < parser.tokens.len() {
        let pos = parser.here();
        return Err(CalcError::new(
            pos,
            "此处存在多余内容（检查是否漏了运算符）",
        ));
    }
    Ok(value)
}

/// calculate 工具参数：要求值的算术表达式。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalculateArgs {
    /// 表达式原文（四则/幂/取余/比较/括号/pi/e，无函数调用）。
    pub expression: String,
}

/// calculate 工具规格（能力 ReadOnly，全档放行）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "calculate".to_string(),
        description: "精确算术求值器：+ - * / % ^ 与一元正负、括号、比较（< <= > >= == !=）、\
             常量 pi/e。整数字面量精确（64 位，溢出报错），小数走 64 位浮点，除法为浮点除（7/2=3.5），\
             % 为截断取余。无函数调用、无副作用。需要任何多步数值计算时用本工具代替心算。"
            .to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "expression": { "type": "string", "description": "算术表达式，如 \"(1 + 2) * 3.5\"、\"2^10\"、\"pi * 2\"" }
            },
            "required": ["expression"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        origin: crate::tools::ToolOrigin::Static,
    }
}

#[cfg(test)]
#[path = "calculate_test.rs"]
mod calculate_test;
