use crate::query::{parse, Query, OrderOperator};
use err_derive::Error;
use std::any::type_name;
use std::collections::HashMap;
use std::str::FromStr;
use nom::Err;

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error(display = "invalid value for field \"{}\", expect {}", field, accepted_type)]
    InvalidValue {
        field: String,
        accepted_type: String,
    },
    #[error(display = "syntax error at position {}", pos)]
    ParseError {
        pos: usize,
    },
    #[error(display = "unsupported operation \"{}\" on field \"{}\"", required_operation, field)]
    UnsupportedOperation {
        field: String,
        required_operation: String,
    },
    #[error(display = "unknown filed \"{}\"", field)]
    UnknownField {
        field: String,
    },
    #[error(display = "empty operation \"{}\" on wildcard field", required_operation)]
    EmptyWildcardOperation {
        required_operation: String,
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct FieldConfig {
    pub field: String,
    pub rename: Option<String>,
    pub type_name: Option<String>,
    pub wildcard: bool,
    pub partial_order: bool,
    pub partial_equal: bool,
    pub use_like: bool,
    pub escape_handler: Option<EscapeHandler>,
}

pub type EscapeHandler = Box<dyn Fn(&str, &FieldConfig) -> Result<String>>;

pub fn escape_quoted_with_converter<T: FromStr>(
    converter: impl Fn(&T) -> String + 'static,
) -> EscapeHandler {
    Box::new(move |input: &str, config: &FieldConfig| {
        let value: T = input.parse()
            .map_err(|_| Error::InvalidValue {
                field: config.field.clone(),
                accepted_type: config.type_name.clone().unwrap_or_else(|| type_name::<T>().into()),
            })?;
        Ok(format!("'{}'", converter(&value).replace("'", "''")))
    })
}

pub fn escape_quoted<T: FromStr + ToString + 'static>() -> EscapeHandler {
    escape_quoted_with_converter(T::to_string)
}

pub fn escape_unquoted_with_converter<T: FromStr>(
    converter: impl Fn(&T) -> String + 'static,
) -> EscapeHandler {
    Box::new(move |input: &str, config: &FieldConfig| {
        let value: T = input.parse()
            .map_err(|_| Error::InvalidValue {
                field: config.field.clone(),
                accepted_type: config.type_name.clone().unwrap_or_else(|| type_name::<T>().into()),
            })?;
        Ok(converter(&value))
    })
}

pub fn escape_unquoted<T: FromStr + ToString + 'static>() -> EscapeHandler {
    escape_unquoted_with_converter(T::to_string)
}

impl FieldConfig {
    pub fn new(field: &str) -> Self {
        Self {
            field: field.into(),
            rename: None,
            type_name: None,
            wildcard: false,
            partial_order: false,
            partial_equal: false,
            use_like: false,
            escape_handler: None,
        }
    }
    pub fn rename(mut self, rename: &str) -> Self {
        self.rename = Some(rename.into());
        self
    }
    pub fn type_name(mut self, type_name: &str) -> Self {
        self.type_name = Some(type_name.into());
        self
    }
    pub fn wildcard(mut self) -> Self {
        self.wildcard = true;
        self
    }
    pub fn partial_order(mut self) -> Self {
        self.partial_order = true;
        self
    }
    pub fn partial_equal(mut self) -> Self {
        self.partial_equal = true;
        self
    }
    pub fn use_like(mut self) -> Self {
        self.use_like = true;
        self
    }
    pub fn escape_handler(mut self, func: EscapeHandler) -> Self {
        self.escape_handler = Some(func);
        self
    }
    pub fn escape(&self, input: &str) -> Result<String> {
        self.escape_handler.as_ref()
            .unwrap_or(&escape_quoted::<String>())
            (input, &self)
    }
}

pub struct QueryConfig {
    fields: HashMap<String, FieldConfig>,
}

impl QueryConfig {
    pub fn new() -> Self {
        Self {
            fields: HashMap::new(),
        }
    }
    pub fn field(mut self, field: FieldConfig) -> Self {
        self.fields.insert(field.field.clone(), field);
        self
    }
    pub fn parse_to_postgres(&self, input: &str) -> Result<Option<String>> {
        Ok(parse(input)
            .map_err(|err| match err {
                Err::Incomplete(..) => Error::ParseError { pos: input.len() },
                Err::Error((rest, ..)) | Err::Failure((rest, ..))
                    => Error::ParseError { pos: input.len() - rest.len() },
            })?
            .1
            .as_ref()
            .map(|x| self.query_to_postgres(x))
            .transpose()?)
    }
    pub fn query_to_postgres(&self, query: &Query) -> Result<String> {
        let result = match query {
            Query::Or { queries } => if queries.is_empty() { "TRUE".into() } else {
                queries.iter()
                    .map(|x| self.query_to_postgres(x))
                    .collect::<Result<Vec<_>>>()?
                    .join(" OR ")
            }
            Query::And { queries } => if queries.is_empty() { "FALSE".into() } else {
                queries.iter()
                    .map(|x| self.query_to_postgres(x))
                    .collect::<Result<Vec<_>>>()?
                    .join(" AND ")
            }
            Query::Not { query } => format!("NOT {}", self.query_to_postgres(query)?),
            Query::Equal { field, value } => match field {
                Some(field) => {
                    let config = self.fields.get(field)
                        .ok_or_else(|| Error::UnknownField { field: field.clone() })?;
                    if config.partial_equal == false {
                        return Err(Error::UnsupportedOperation {
                            field: field.clone(),
                            required_operation: "equal".into(),
                        });
                    }
                    let value = config.escape(value)?;
                    let rename = config.rename.as_ref().unwrap_or(field);
                    if config.use_like {
                        let value = value
                            .replace("^", "^^")
                            .replace("%", "^%")
                            .replace("_", "^_");
                        format!("{} ILIKE '%' || {} || '%' ESCAPE '^'", rename, value)
                    } else {
                        format!("{} = {}", rename, value)
                    }
                }
                None => {
                    let queries = self.fields.values()
                        .filter(|x| x.wildcard && x.partial_equal)
                        .map(|config| {
                            let value = config.escape(value)?;
                            let rename = config.rename.as_ref().unwrap_or(&config.field);
                            Ok(if config.use_like {
                                let value = value
                                    .replace("^", "^^")
                                    .replace("%", "^%")
                                    .replace("_", "^_");
                                format!("{} ILIKE '%' || {} || '%' ESCAPE '^'", rename, value)
                            } else {
                                format!("{} = {}", rename, value)
                            })
                        })
                        .flat_map(Result::ok)
                        .collect::<Vec<_>>();
                    if queries.is_empty() {
                        return Err(Error::EmptyWildcardOperation {
                            required_operation: "equal".into(),
                        });
                    }
                    queries.join(" OR ")
                }
            }
            Query::Order { field, operator, value } => {
                let operator = match operator {
                    OrderOperator::Lte => "<=",
                    OrderOperator::Gte => ">=",
                    OrderOperator::Lt => "<",
                    OrderOperator::Gt => ">",
                };
                match field {
                    Some(field) => {
                        let config = self.fields.get(field)
                            .ok_or_else(|| Error::UnknownField { field: field.clone() })?;
                        if config.partial_order == false {
                            return Err(Error::UnsupportedOperation {
                                field: field.clone(),
                                required_operation: "order".into(),
                            });
                        }
                        let value = config.escape(value)?;
                        let rename = config.rename.as_ref().unwrap_or(field);
                        format!("{} {} {}", rename, operator, value)
                    }
                    None => {
                        let queries = self.fields.values()
                            .filter(|x| x.wildcard && x.partial_order)
                            .map(|config| {
                                let value = config.escape(value)?;
                                let rename = config.rename.as_ref().unwrap_or(&config.field);
                                Ok(format!("{} {} {}", rename, operator, value))
                            })
                            .filter_map(Result::ok)
                            .collect::<Vec<_>>();
                        if queries.is_empty() {
                            return Err(Error::EmptyWildcardOperation {
                                required_operation: "order".into(),
                            });
                        }
                        queries.join(" OR ")
                    }
                }
            }
        };
        Ok(format!("({})", result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type UtcDateTime = chrono::DateTime<chrono::Utc>;

    #[test]
    pub fn escape_to_string_test() {
        assert_eq!(
            FieldConfig::new("").escape("abc'inject"),
            Ok("'abc''inject'".into()));
    }

    #[test]
    pub fn escape_to_chrono_test() {
        assert_eq!(
            FieldConfig::new("")
                .escape_handler(escape_quoted_with_converter(UtcDateTime::to_rfc3339))
                .escape("2014-11-28T12:00:09Z"),
            Ok("'2014-11-28T12:00:09+00:00'".into()));
        assert_eq!(
            FieldConfig::new("a")
                .escape_handler(escape_quoted_with_converter(UtcDateTime::to_rfc3339))
                .type_name("DateTime")
                .escape("abc"),
            Err(Error::InvalidValue {
                field: "a".into(),
                accepted_type: "DateTime".into(),
            }));
    }

    #[test]
    pub fn escape_to_number_test() {
        assert_eq!(
            FieldConfig::new("")
                .escape_handler(escape_unquoted::<i32>())
                .escape("-1"),
            Ok("-1".into()));
        assert_eq!(
            FieldConfig::new("")
                .escape_handler(escape_unquoted::<u32>())
                .escape("-1"),
            Err(Error::InvalidValue {
                field: "".into(),
                accepted_type: "u32".into(),
            }));
    }

    #[test]
    pub fn generator_test1() {
        let generator = QueryConfig::new()
            .field(FieldConfig::new("id")
                .wildcard()
                .partial_equal()
                .partial_order()
                .escape_handler(escape_unquoted::<i32>())
            )
            .field(FieldConfig::new("text")
                .wildcard()
                .rename("\"text\"")
                .use_like()
                .partial_equal()
            );
        assert_eq!(
            generator.parse_to_postgres("id > 1 and (id < 1 id: 1)"),
            Ok(Some("((id > 1) AND ((id < 1) OR (id = 1)))".into()))
        );
        assert_eq!(
            generator.parse_to_postgres("\n"),
            Ok(None)
        );
        assert_eq!(
            generator.parse_to_postgres("* > 2"),
            Ok(Some("(id > 2)".into()))
        );
        let result = generator.parse_to_postgres("1");
        assert!(
            result == Ok(Some("(id = 1 OR \"text\" ILIKE '%' || '1' || '%' ESCAPE '^')".into()))
            || result == Ok(Some("(\"text\" ILIKE '%' || '1' || '%' ESCAPE '^' OR id = 1)".into()))
        );
        assert_eq!(
            generator.parse_to_postgres("ab%c"),
            Ok(Some("(\"text\" ILIKE '%' || 'ab^%c' || '%' ESCAPE '^')".into()))
        );
    }
}

