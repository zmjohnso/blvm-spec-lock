//! Lexer for spec condition strings
//!
//! Tokenizes mathematical/spec conditions from the Orange Paper so they can be
//! translated to parseable Rust expressions for Z3 verification.

/// Token produced by the lexer
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Ident(String),
    Number(String),
    Op(String),      // >=, <=, ==, !=, >, <, =>, &&, ||
    Lparen,
    Rparen,
    Comma,
    Dot,
}

/// Lexer for spec condition strings
pub struct Lexer {
    input: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Lexer {
            input: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn peek_n(&self, n: usize) -> Option<char> {
        self.input.get(self.pos + n).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_whitespace(&mut self) {
        while self.peek().map_or(false, |c| c.is_whitespace()) {
            self.advance();
        }
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '\'' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_number(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_backslash_command(&mut self) -> Option<String> {
        if self.peek() != Some('\\') {
            return None;
        }
        self.advance();
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphabetic() || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        Some(s)
    }

    fn read_curly_content(&mut self) -> Option<String> {
        if self.peek() != Some('{') {
            return None;
        }
        self.advance();
        let mut depth = 1;
        let mut s = String::new();
        while let Some(c) = self.advance() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(s);
                    }
                }
                _ => s.push(c),
            }
        }
        None
    }

    /// Lex the next token
    pub fn next_token(&mut self) -> Option<Token> {
        self.skip_whitespace();
        let c = self.peek()?;

        // Two-char operators
        if c == '=' && self.peek_n(1) == Some('=') {
            self.advance();
            self.advance();
            return Some(Token::Op("==".to_string()));
        }
        if c == '>' && self.peek_n(1) == Some('=') {
            self.advance();
            self.advance();
            return Some(Token::Op(">=".to_string()));
        }
        if c == '<' && self.peek_n(1) == Some('=') {
            self.advance();
            self.advance();
            return Some(Token::Op("<=".to_string()));
        }
        if c == '!' && self.peek_n(1) == Some('=') {
            self.advance();
            self.advance();
            return Some(Token::Op("!=".to_string()));
        }
        if c == '=' && self.peek_n(1) == Some('>') {
            self.advance();
            self.advance();
            return Some(Token::Op("=>".to_string()));
        }
        if c == '&' && self.peek_n(1) == Some('&') {
            self.advance();
            self.advance();
            return Some(Token::Op("&&".to_string()));
        }
        if c == '|' && self.peek_n(1) == Some('|') {
            self.advance();
            self.advance();
            return Some(Token::Op("||".to_string()));
        }

        // Single-char
        if c == '(' {
            self.advance();
            return Some(Token::Lparen);
        }
        if c == ')' {
            self.advance();
            return Some(Token::Rparen);
        }
        if c == ',' {
            self.advance();
            return Some(Token::Comma);
        }
        if c == '.' {
            self.advance();
            return Some(Token::Dot);
        }
        if c == '>' || c == '<' {
            self.advance();
            return Some(Token::Op(c.to_string()));
        }
        if c == '=' && self.peek_n(1) != Some('=') && self.peek_n(1) != Some('>') {
            self.advance();
            return Some(Token::Op("==".to_string())); // spec "=" means equality
        }

        // LaTeX \text{Name} -> Ident
        if c == '\\' {
            if let Some(cmd) = self.read_backslash_command() {
                match cmd.as_str() {
                    "text" => {
                        if self.peek() == Some('{') {
                            if let Some(inner) = self.read_curly_content() {
                                return Some(Token::Ident(inner));
                            }
                        }
                    }
                    "implies" => return Some(Token::Op("=>".to_string())),
                    "iff" => return Some(Token::Op("==".to_string())),
                    "land" => return Some(Token::Op("&&".to_string())),
                    "lor" => return Some(Token::Op("||".to_string())),
                    "geq" => return Some(Token::Op(">=".to_string())),
                    "leq" => return Some(Token::Op("<=".to_string())),
                    "neq" => return Some(Token::Op("!=".to_string())),
                    "neg" => return Some(Token::Op("!".to_string())),
                    "lfloor" | "rfloor" | "mathbb" => {
                        if self.peek() == Some('{') {
                            let _ = self.read_curly_content();
                        }
                        return self.next_token();
                    }
                    _ => {}
                }
            }
        }

        // Ident or number
        if c.is_alphabetic() || c == '_' {
            return Some(Token::Ident(self.read_ident()));
        }
        if c.is_ascii_digit() {
            return Some(Token::Number(self.read_number()));
        }

        // Skip unknown (e.g. $, other LaTeX)
        self.advance();
        self.next_token()
    }

    /// Lex all tokens
    pub fn lex(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        while let Some(t) = self.next_token() {
            tokens.push(t);
        }
        tokens
    }
}

/// Convert tokens back to a Rust-like expression string
pub fn tokens_to_rust_expr(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (i, t) in tokens.iter().enumerate() {
        match t {
            Token::Ident(s) => {
                let rust = spec_ident_to_rust(s);
                out.push_str(&rust);
            }
            Token::Number(s) => out.push_str(s),
            Token::Op(s) => out.push_str(s),
            Token::Lparen => out.push('('),
            Token::Rparen => out.push(')'),
            Token::Comma => out.push_str(", "),
            Token::Dot => out.push('.'),
        }
        if i + 1 < tokens.len() {
            let next = &tokens[i + 1];
            if !matches!(next, Token::Rparen | Token::Comma | Token::Op(_))
                && !matches!(t, Token::Lparen | Token::Comma | Token::Op(_))
            {
                out.push(' ');
            }
        }
    }
    out
}

fn spec_ident_to_rust(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "result".to_string();
    }
    match s {
        "result" | "true" | "false" => s.to_string(),
        _ => {
            let s = s.replace("script'", "script_out").replace("pattern'", "pattern_out");
            if s.contains('(') {
                s.split('(').next().unwrap_or(&s).to_string()
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex_simple() {
        let mut lex = Lexer::new("result >= 0");
        let tokens = lex.lex();
        assert_eq!(
            tokens,
            vec![
                Token::Ident("result".to_string()),
                Token::Op(">=".to_string()),
                Token::Number("0".to_string()),
            ]
        );
    }

    #[test]
    fn test_lex_implies() {
        let mut lex = Lexer::new("h = 0 => result == INITIAL_SUBSIDY");
        let tokens = lex.lex();
        assert!(tokens.contains(&Token::Op("=>".to_string())));
    }
}
