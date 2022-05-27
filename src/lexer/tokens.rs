use crate::lexer::Bracket::{Angle, Curly, Round, Square};
use crate::lexer::Comment::{Docstring, Regular};
use crate::lexer::ErrorKind::{
    FloatParsing, IncompleteEscape, UnknownChar, UnknownEscape, UnterminatedStrLiteral,
};
use crate::lexer::Keyword::{
    And, Arr, Bool, False, Float as FloatKeyword, Fn, For, If, Int as IntKeyword, Map, Not, Or,
    Set, Str as StrKeyword, True, While,
};
use crate::lexer::Literal::{Float as FloatLiteral, Int as IntLiteral, Str as StrLiteral};
use crate::lexer::Punctuation::{
    Ampersand, Asterisk, At, Backslash, Caret, CloseBracket, Colon, Comma, Dash, Dollar, Dot,
    Equals, Exclamation, Hashtag, Newline, OpenBracket, Percent, Pipe, Plus, Question, SingleQuote,
    Slash, Tilde,
};
use crate::lexer::{Error, Result, Token};
use std::iter::Peekable;

#[derive(Debug)]
pub struct Tokens<'a> {
    pub(crate) unparsed: &'a str,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Result<Token>;

    fn next(&mut self) -> Option<Self::Item> {
        // skip whitespace
        let i = self
            .unparsed
            .find(|c: char| !c.is_ascii_whitespace() || c == '\n')?;
        self.unparsed = &self.unparsed[i..];

        let mut iter = self.unparsed.chars().peekable();

        let cur = iter.next()?;
        let next = iter.peek();
        let (token_or_err, source_len) = match (cur, next) {
            ('/', Some('/')) => self.read_comment(),
            ('"', _) => self.read_string_literal(),

            ('&', _) => (Ok(Token::Punctuation(Ampersand)), 1),
            ('*', _) => (Ok(Token::Punctuation(Asterisk)), 1),
            ('@', _) => (Ok(Token::Punctuation(At)), 1),
            ('\\', _) => (Ok(Token::Punctuation(Backslash)), 1),
            ('^', _) => (Ok(Token::Punctuation(Caret)), 1),
            (':', _) => (Ok(Token::Punctuation(Colon)), 1),
            (',', _) => (Ok(Token::Punctuation(Comma)), 1),
            ('-', _) => (Ok(Token::Punctuation(Dash)), 1),
            ('$', _) => (Ok(Token::Punctuation(Dollar)), 1),
            ('.', _) => (Ok(Token::Punctuation(Dot)), 1),
            ('=', _) => (Ok(Token::Punctuation(Equals)), 1),
            ('!', _) => (Ok(Token::Punctuation(Exclamation)), 1),
            ('#', _) => (Ok(Token::Punctuation(Hashtag)), 1),
            ('\n', _) => (Ok(Token::Punctuation(Newline)), 1),
            ('%', _) => (Ok(Token::Punctuation(Percent)), 1),
            ('|', _) => (Ok(Token::Punctuation(Pipe)), 1),
            ('+', _) => (Ok(Token::Punctuation(Plus)), 1),
            ('?', _) => (Ok(Token::Punctuation(Question)), 1),
            ('\'', _) => (Ok(Token::Punctuation(SingleQuote)), 1),
            ('/', _) => (Ok(Token::Punctuation(Slash)), 1),
            ('~', _) => (Ok(Token::Punctuation(Tilde)), 1),

            ('<', _) => (Ok(Token::Punctuation(OpenBracket(Angle))), 1),
            ('>', _) => (Ok(Token::Punctuation(CloseBracket(Angle))), 1),
            ('{', _) => (Ok(Token::Punctuation(OpenBracket(Curly))), 1),
            ('}', _) => (Ok(Token::Punctuation(CloseBracket(Curly))), 1),
            ('(', _) => (Ok(Token::Punctuation(OpenBracket(Round))), 1),
            (')', _) => (Ok(Token::Punctuation(CloseBracket(Round))), 1),
            ('[', _) => (Ok(Token::Punctuation(OpenBracket(Square))), 1),
            (']', _) => (Ok(Token::Punctuation(CloseBracket(Square))), 1),

            (c, _) if c.is_ascii_digit() => self.read_numeric_literal(),
            (c, _) if c.is_alphabetic() || c == '_' => self.read_identifier_or_kw(),

            (c, _) => (Err(Error::new(UnknownChar(c))), c.len_utf8()),
        };

        self.unparsed = &self.unparsed[source_len..];
        Some(token_or_err)
    }
}

impl<'a> Tokens<'a> {
    fn read_comment(&mut self) -> (Result<Token>, usize) {
        let n_slashes = self
            .unparsed
            .find(|c| c != '/')
            .unwrap_or(self.unparsed.len());

        let source_len = self.unparsed.find('\n').unwrap_or(self.unparsed.len());

        let comment_body = self.unparsed[n_slashes..source_len].trim().to_string();

        match n_slashes {
            3 => (Ok(Token::Comment(Docstring(comment_body))), source_len),
            _ => (Ok(Token::Comment(Regular(comment_body))), source_len),
        }
    }

    fn read_string_literal(&mut self) -> (Result<Token>, usize) {
        let mut contents = String::new();

        // We skip the first quote
        let mut iter = self.unparsed.chars().enumerate().skip(1).peekable();

        let mut parsing_error = None;

        loop {
            if let Some((i, c)) = iter.next() {
                match c {
                    '"' => {
                        let default = Ok(Token::Literal(StrLiteral(contents)));
                        return (parsing_error.unwrap_or(default), i + 1);
                    }
                    '\n' => return (Err(Error::new(UnterminatedStrLiteral)), i),
                    '\\' => match Self::parse_escape_in_str_literal(&mut iter) {
                        Ok(c) => contents.push(c),
                        Err(e) => parsing_error = Some(Err(e)),
                    },
                    c => contents.push(c),
                }
            } else {
                return (Err(Error::new(UnterminatedStrLiteral)), self.unparsed.len());
            }
        }
    }

    fn parse_escape_in_str_literal(
        iter: &mut Peekable<impl Iterator<Item = (usize, char)>>,
    ) -> Result<char> {
        match iter.next() {
            Some((_, c)) => match c {
                'n' => Ok('\n'),
                'r' => Ok('\r'),
                't' => Ok('\t'),
                '\\' => Ok('\\'),
                '0' => Ok('\0'),
                '"' => Ok('"'),
                'u' => Self::parse_hex_str(Self::take_n_hex_chars(iter, 4)?),
                'x' => Self::parse_hex_str(Self::take_n_hex_chars(iter, 2)?),
                c => Err(Error::new(UnknownEscape(c))),
            },
            None => Err(Error::new(UnterminatedStrLiteral)),
        }
    }

    fn take_n_hex_chars(
        iter: &mut Peekable<impl Iterator<Item = (usize, char)>>,
        n: usize,
    ) -> Result<String> {
        let mut hex_chars = String::new();
        for _ in 0..n {
            if let Some(&(_, c)) = iter.peek() {
                if c.is_ascii_hexdigit() {
                    // Unwrapping next here is safe because we peeked above
                    hex_chars.push(iter.next().unwrap().1);
                } else {
                    return Err(Error::new(IncompleteEscape));
                }
            } else {
                return Err(Error::new(UnterminatedStrLiteral));
            }
        }
        Ok(hex_chars)
    }

    fn parse_hex_str(hex_str: String) -> Result<char> {
        Ok(char::from_u32(u32::from_str_radix(&hex_str, 16).unwrap()).unwrap())
    }

    fn read_numeric_literal(&mut self) -> (Result<Token>, usize) {
        // While we never check the very first digit, we know it is going to be a digit because otherwise this function would never have been called
        let source_len = self
            .unparsed
            .chars()
            .zip(self.unparsed.chars().skip(1))
            .enumerate()
            .find(|&(_, (prev, cur))| {
                !(cur.is_ascii_digit()
                    || cur == '.'
                    || cur == 'E'
                    || cur == 'e'
                    || ((cur == '-' || cur == '+') && (prev == 'E' || prev == 'e')))
            })
            .map(|(i, _)| i + 1)
            .unwrap_or(self.unparsed.len());

        let num = &self.unparsed[..source_len];

        let res = if num.contains(|c| c == 'E' || c == 'e' || c == '.') {
            match num.parse() {
                Ok(n) => Ok(Token::Literal(FloatLiteral(n))),
                _ => Err(Error::new(FloatParsing(num.to_string()))),
            }
        } else {
            let n = num.parse().unwrap();
            Ok(Token::Literal(IntLiteral(n)))
        };

        (res, source_len)
    }

    fn read_identifier_or_kw(&mut self) -> (Result<Token>, usize) {
        let source_len = self
            .unparsed
            .find(|c: char| !(c.is_alphabetic() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(self.unparsed.len());

        let name = &self.unparsed[..source_len];

        let token = match name {
            "arr" => Token::Keyword(Arr),
            "bool" => Token::Keyword(Bool),
            "float" => Token::Keyword(FloatKeyword),
            "int" => Token::Keyword(IntKeyword),
            "map" => Token::Keyword(Map),
            "set" => Token::Keyword(Set),
            "str" => Token::Keyword(StrKeyword),

            "and" => Token::Keyword(And),
            "false" => Token::Keyword(False),
            "not" => Token::Keyword(Not),
            "or" => Token::Keyword(Or),
            "true" => Token::Keyword(True),

            "fn" => Token::Keyword(Fn),
            "for" => Token::Keyword(For),
            "if" => Token::Keyword(If),
            "while" => Token::Keyword(While),

            _ => Token::Identifier(name.to_string()),
        };
        (Ok(token), source_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    macro_rules! assert_source_has_expected_output {
        ($source:expr, $expected:expr) => {{
            let tokens: Vec<_> = Tokens { unparsed: $source }.collect();
            assert_eq!(tokens, $expected)
        }};
    }

    macro_rules! assert_source_all_ok_and_has_expected_output {
        ($source:expr, $expected:expr) => {{
            let expected: Vec<_> = $expected.into_iter().map(|t| Ok(t)).collect();
            assert_source_has_expected_output!($source, expected)
        }};
    }

    macro_rules! prop_assert_source_has_expected_output {
        ($source:expr, $expected:expr) => {{
            let tokens: Vec<_> = Tokens { unparsed: $source }.collect();
            prop_assert_eq!(tokens, $expected)
        }};
    }

    macro_rules! prop_assert_source_all_ok_and_has_expected_output {
        ($source:expr, $expected:expr) => {{
            let expected: Vec<_> = $expected.into_iter().map(|t| Ok(t)).collect();
            prop_assert_source_has_expected_output!($source, expected)
        }};
    }

    #[test]
    fn err_given_unknown_str_escape() {
        let source = r#""\e""#;
        let expected = vec![Err(Error::new(UnknownEscape('e')))];
        assert_source_has_expected_output!(&source.to_string(), expected)
    }

    #[test]
    fn err_given_incomplete_str_unicode_escape() {
        let source = r#""\u3b9""#;
        let expected = vec![Err(Error::new(IncompleteEscape))];
        assert_source_has_expected_output!(&source.to_string(), expected)
    }

    #[test]
    fn err_given_incomplete_str_hex_escape() {
        let source = r#""\x9""#;
        let expected = vec![Err(Error::new(IncompleteEscape))];
        assert_source_has_expected_output!(&source.to_string(), expected)
    }

    #[test]
    fn err_given_unterminated_str_literal() {
        let source = "\"test\n\"";

        let expected = vec![
            Err(Error::new(UnterminatedStrLiteral)),
            Ok(Token::Punctuation(Newline)),
            Err(Error::new(UnterminatedStrLiteral)),
        ];

        assert_source_has_expected_output!(&source.to_string(), expected)
    }

    #[test]
    fn err_given_unknown_char() {
        let source = '∂';
        let expected = vec![Err(Error::new(UnknownChar(source)))];
        assert_source_has_expected_output!(&source.to_string(), expected)
    }

    #[test]
    fn err_given_float_with_multiple_e() {
        let source = "1.2312E-33333E+9999";
        let expected = vec![Err(Error::new(FloatParsing(source.to_string())))];
        assert_source_has_expected_output!(source, expected)
    }

    #[test]
    fn err_given_float_with_consecutive_e() {
        let source = "1.2312Ee9999";
        let expected = vec![Err(Error::new(FloatParsing(source.to_string())))];
        assert_source_has_expected_output!(source, expected)
    }

    #[test]
    fn err_given_float_with_e_pm() {
        let source = "1.2312E+-9999";

        let expected = vec![
            Err(Error::new(FloatParsing("1.2312E+".to_string()))),
            Ok(Token::Punctuation(Dash)),
            Ok(Token::Literal(IntLiteral(9999))),
        ];

        assert_source_has_expected_output!(source, expected)
    }

    #[test]
    fn err_given_float_with_multiple_decimals() {
        let source = "123.456.789";
        let expected = vec![Err(Error::new(FloatParsing(source.to_string())))];
        assert_source_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_regular_comment() {
        let source = "//      It is way too late    ";
        let expected = vec![Token::Comment(Regular("It is way too late".to_string()))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_more_than_three_slashes_as_regular_comment() {
        let source = "///////////  Thomas is the best!    ";
        let expected = vec![Token::Comment(Regular("Thomas is the best!".to_string()))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_docstring_comment() {
        let source = "/// Max is the best! ";
        let expected = vec![Token::Comment(Docstring("Max is the best!".to_string()))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_multiline_comment() {
        let source = "/// Line 1\n// Line 2";

        let expected = vec![
            Token::Comment(Docstring("Line 1".to_string())),
            Token::Punctuation(Newline),
            Token::Comment(Regular("Line 2".to_string())),
        ];

        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_escape_characters() {
        let source = r#""\\\n\r\t\0\"""#;
        let expected = vec![Token::Literal(StrLiteral("\\\n\r\t\0\"".to_string()))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_hex_escape_characters() {
        // print(''.join(["\\" + str(hex(ord(c)))[1:] for c in 'flick']))
        let source = r#""\x66\x6c\x69\x63\x6b""#;
        let expected = vec![Token::Literal(StrLiteral("flick".to_string()))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_unicode_escape_characters() {
        let source = r#""\u2702\u0046\u002f""#;
        let expected = vec![Token::Literal(StrLiteral("✂F/".to_string()))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_short_program() {
        let source = "call(3)\nprint(5)";

        let expected = vec![
            Token::Identifier("call".to_string()),
            Token::Punctuation(OpenBracket(Round)),
            Token::Literal(IntLiteral(3)),
            Token::Punctuation(CloseBracket(Round)),
            Token::Punctuation(Newline),
            Token::Identifier("print".to_string()),
            Token::Punctuation(OpenBracket(Round)),
            Token::Literal(IntLiteral(5)),
            Token::Punctuation(CloseBracket(Round)),
        ];

        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_map_statement() {
        let source = "map<str, int> m = {\"hi\": 2, \"bye\": 100}";

        let expected = vec![
            Token::Keyword(Map),
            Token::Punctuation(OpenBracket(Angle)),
            Token::Keyword(StrKeyword),
            Token::Punctuation(Comma),
            Token::Keyword(IntKeyword),
            Token::Punctuation(CloseBracket(Angle)),
            Token::Identifier("m".to_string()),
            Token::Punctuation(Equals),
            Token::Punctuation(OpenBracket(Curly)),
            Token::Literal(StrLiteral("hi".to_string())),
            Token::Punctuation(Colon),
            Token::Literal(IntLiteral(2)),
            Token::Punctuation(Comma),
            Token::Literal(StrLiteral("bye".to_string())),
            Token::Punctuation(Colon),
            Token::Literal(IntLiteral(100)),
            Token::Punctuation(CloseBracket(Curly)),
        ];

        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_keyword() {
        let source = "if";
        let expected = vec![Token::Keyword(If)];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_floats_in_scientific_notation_with_big_e() {
        let source = "1.1E12";
        let expected = vec![Token::Literal(FloatLiteral(1.1E12))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_floats_in_scientific_notation_with_small_e() {
        let source = "3.9993e12";
        let expected = vec![Token::Literal(FloatLiteral(3.9993e12))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_floats_in_scientific_notation_with_positive_e() {
        let source = "123456789E+11";
        let expected = vec![Token::Literal(FloatLiteral(123456789E+11))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_floats_in_scientific_notation_with_negative_e() {
        let source = "6.67430e-11";
        let expected = vec![Token::Literal(FloatLiteral(6.67430e-11))];
        assert_source_all_ok_and_has_expected_output!(source, expected)
    }

    #[test]
    fn parses_identifier_with_non_ascii_chars() {
        let source = "çaí";
        let expected = vec![Token::Identifier(source.to_string())];
        assert_source_all_ok_and_has_expected_output!(source, expected);
    }

    proptest! {
        #[test]
        fn parses_numbers(source in any::<usize>().prop_map(|n| n.to_string())) {
            let expected = vec![Token::Literal(IntLiteral(source.parse().unwrap()))];
            prop_assert_source_all_ok_and_has_expected_output!(&source, expected)
        }

        #[test]
        fn parses_float(mut source in proptest::num::f64::POSITIVE.prop_map(|f| f.to_string())) {
            if !source.contains(|c| c == '.' || c == 'E' || c == 'e') {
                source.push('.'); // To avoid getting parsed as int
            }

            let expected = vec![Token::Literal(FloatLiteral(source.parse().unwrap()))];
            prop_assert_source_all_ok_and_has_expected_output!(&source, expected)
        }

        #[test]
        fn parses_identifiers(source in "[a-zA-Z_][a-zA-Z0-9_]*") {
            let expected = vec![Token::Identifier(source.clone())];
            prop_assert_source_all_ok_and_has_expected_output!(&source, expected)
        }

        #[test]
        fn parses_strings_without_escapes(source in r#""[a-zA-Z0-9_]*""#) {
            let expected = vec![Token::Literal(StrLiteral(source[1..source.len()-1].to_string()))];
            prop_assert_source_all_ok_and_has_expected_output!(&source, expected)
        }
    }
}
