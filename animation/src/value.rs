use serde::{Deserialize, Serialize};

/// Runtime value tag for a typed binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Number,
    Integer,
    Boolean,
    String,
    Vec3,
    Vec4,
    Quaternion,
    Weights,
}

/// A project-owned animation value. Quaternions use `[x, y, z, w]`; morph weights retain their authored
/// width so a binding can validate against the target's declared morph count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimValue {
    Number(f64),
    Integer(i64),
    Boolean(bool),
    String(String),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Quaternion([f64; 4]),
    Weights(Vec<f64>),
}

impl AnimValue {
    #[must_use]
    pub const fn kind(&self) -> ValueKind {
        match self {
            Self::Number(_) => ValueKind::Number,
            Self::Integer(_) => ValueKind::Integer,
            Self::Boolean(_) => ValueKind::Boolean,
            Self::String(_) => ValueKind::String,
            Self::Vec3(_) => ValueKind::Vec3,
            Self::Vec4(_) => ValueKind::Vec4,
            Self::Quaternion(_) => ValueKind::Quaternion,
            Self::Weights(_) => ValueKind::Weights,
        }
    }

    #[must_use]
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Number(value) => value.is_finite(),
            Self::Vec3(value) => value.iter().all(|part| part.is_finite()),
            Self::Vec4(value) | Self::Quaternion(value) => {
                value.iter().all(|part| part.is_finite())
            }
            Self::Weights(value) => value.iter().all(|part| part.is_finite()),
            Self::Integer(_) | Self::Boolean(_) | Self::String(_) => true,
        }
    }

    #[must_use]
    pub fn is_interpolatable(&self) -> bool {
        !matches!(self, Self::Boolean(_) | Self::String(_))
    }

    #[must_use]
    pub fn normalized_quaternion(&self) -> Option<Self> {
        let Self::Quaternion(value) = self else {
            return None;
        };
        let length_squared = value.iter().map(|part| part * part).sum::<f64>();
        if !length_squared.is_finite() || length_squared <= 1.0e-24 {
            return None;
        }
        let inverse_length = length_squared.sqrt().recip();
        Some(Self::Quaternion(value.map(|part| part * inverse_length)))
    }

    pub(crate) fn canonicalize(self) -> Self {
        match self.normalized_quaternion() {
            Some(normalized) => normalized,
            None => self,
        }
    }
}
