use oxihome_protocol::{
    Access, Color, ColorModel, CommandOp, EntityDescriptor, ErrorCode, Value, ValueType,
};

/// Why a command was refused; becomes an `Error` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    pub code: ErrorCode,
    pub detail: String,
}

impl Rejection {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Checks a command against the declared entities: the channel exists, its `Access` allows the
/// operation and the value fits its `ValueType`.
pub fn check(
    entities: &[EntityDescriptor],
    entity: u8,
    channel: u8,
    op: &CommandOp,
) -> Result<(), Rejection> {
    let Some(descriptor) = entities.iter().find(|declared| declared.id == entity) else {
        return Err(Rejection::new(
            ErrorCode::UnknownEntity,
            format!("unknown entity {entity}"),
        ));
    };
    let Some(declared) = descriptor
        .channels
        .iter()
        .find(|declared| declared.id == channel)
    else {
        return Err(Rejection::new(
            ErrorCode::UnknownChannel,
            format!("entity {entity} has no channel {channel}"),
        ));
    };
    let not_allowed = || {
        Rejection::new(
            ErrorCode::InvalidOperation,
            format!("{op:?} not allowed on entity {entity} channel {channel}"),
        )
    };
    match op {
        CommandOp::Set(value) => {
            if !matches!(declared.access, Access::Write | Access::ReadWrite) {
                return Err(not_allowed());
            }
            let Some(value_type) = &declared.value else {
                return Err(not_allowed());
            };
            fits(value_type, value)
        }
        CommandOp::Invoke(value) => {
            if declared.access != Access::Trigger {
                return Err(not_allowed());
            }
            match (&declared.value, value) {
                (None, None) => Ok(()),
                (None, Some(value)) => Err(Rejection::new(
                    ErrorCode::InvalidValue,
                    format!("takes no value, got {value:?}"),
                )),
                (Some(value_type), None) => Err(Rejection::new(
                    ErrorCode::InvalidValue,
                    format!("expected {}, got no value", type_name(value_type)),
                )),
                (Some(value_type), Some(value)) => fits(value_type, value),
            }
        }
    }
}

fn fits(value_type: &ValueType, value: &Value) -> Result<(), Rejection> {
    let invalid = |detail: String| Err(Rejection::new(ErrorCode::InvalidValue, detail));
    match (value_type, value) {
        (ValueType::Bool, Value::Bool(_)) | (ValueType::Bytes, Value::Bytes(_)) => Ok(()),
        (ValueType::Int { min, max, .. }, Value::Int(number)) => {
            if (min..=max).contains(&number) {
                Ok(())
            } else {
                invalid(format!("{number} outside {min}..={max}"))
            }
        }
        (ValueType::Float { min, max, .. }, Value::Float(number)) => {
            if (min..=max).contains(&number) {
                Ok(())
            } else {
                invalid(format!("{number} outside {min}..={max}"))
            }
        }
        (ValueType::Enum(options), Value::Enum(option)) => {
            if options.contains(option) {
                Ok(())
            } else {
                invalid(format!("\"{option}\" is not one of {}", options.join(", ")))
            }
        }
        (ValueType::Color(model), Value::Color(color)) => {
            let matches = matches!(
                (model, color),
                (ColorModel::Rgb, Color::Rgb { .. }) | (ColorModel::Hsv, Color::Hsv { .. })
            );
            if matches {
                Ok(())
            } else {
                invalid(format!("expected a {model:?} color, got {color:?}"))
            }
        }
        (ValueType::Text { max_length }, Value::Text(text)) => {
            if text.len() <= usize::from(*max_length) {
                Ok(())
            } else {
                invalid(format!(
                    "text of {} bytes, at most {max_length}",
                    text.len()
                ))
            }
        }
        (value_type, value) => {
            invalid(format!("expected {}, got {value:?}", type_name(value_type)))
        }
    }
}

fn type_name(value_type: &ValueType) -> &'static str {
    match value_type {
        ValueType::Bool => "Bool",
        ValueType::Int { .. } => "Int",
        ValueType::Float { .. } => "Float",
        ValueType::Enum(_) => "Enum",
        ValueType::Color(_) => "Color",
        ValueType::Text { .. } => "Text",
        ValueType::Bytes => "Bytes",
    }
}
