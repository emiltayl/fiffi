use crate::types::{FfiTypeLayout, ScalarType, TypeRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RegisterSummary {
    layout: FfiTypeLayout,
    integer_bytes: u16,
    sse_bytes: u16,
}

impl RegisterSummary {
    fn for_type(ty: TypeRef<'_>) -> Self {
        match ty {
            TypeRef::Scalar(scalar) => Self::for_scalar(scalar),
            TypeRef::Struct(fields) => {
                let mut summary = Self::empty();

                for field in fields {
                    let child = Self::for_type(TypeRef::from(field));
                    let offset = summary.layout.append_field(child.layout);
                    summary.include_at(child, offset);
                }

                summary.layout.pad_to_alignment();
                summary
            }
            TypeRef::Union(variants) => {
                let mut summary = Self::empty();

                for variant in variants {
                    let child = Self::for_type(TypeRef::from(variant));
                    summary.layout.include_variant(child.layout);
                    summary.integer_bytes |= child.integer_bytes;
                    summary.sse_bytes |= child.sse_bytes;
                }

                summary.layout.pad_to_alignment();
                summary
            }
        }
    }

    fn empty() -> Self {
        Self {
            layout: FfiTypeLayout { align: 1, size: 0 },
            integer_bytes: 0,
            sse_bytes: 0,
        }
    }

    fn for_scalar(scalar: ScalarType) -> Self {
        let layout = TypeRef::Scalar(scalar).layout();
        debug_assert!((1..=16).contains(&layout.size));
        let occupied_bytes = u16::MAX >> (16 - layout.size);

        match scalar {
            ScalarType::I8
            | ScalarType::U8
            | ScalarType::I16
            | ScalarType::U16
            | ScalarType::I32
            | ScalarType::U32
            | ScalarType::I64
            | ScalarType::U64
            | ScalarType::I128
            | ScalarType::U128
            | ScalarType::Isize
            | ScalarType::Usize
            | ScalarType::Pointer => Self {
                layout,
                integer_bytes: occupied_bytes,
                sse_bytes: 0,
            },
            ScalarType::F32 | ScalarType::F64 => Self {
                layout,
                integer_bytes: 0,
                sse_bytes: occupied_bytes,
            },
        }
    }

    fn include_at(&mut self, child: Self, offset: usize) {
        // Classification only summarizes nonempty types of at most 16 bytes.
        debug_assert!(offset < 16);
        debug_assert!(child.layout.size <= 16 - offset);
        self.integer_bytes |= child.integer_bytes << offset;
        self.sse_bytes |= child.sse_bytes << offset;
    }

    fn eightbyte_class(self, byte_mask: u16) -> EightbyteClass {
        if self.integer_bytes & byte_mask != 0 {
            EightbyteClass::Integer
        } else if self.sse_bytes & byte_mask != 0 {
            EightbyteClass::Sse
        } else {
            EightbyteClass::NoClass
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueClass {
    Integer,
    IntegerInteger,
    IntegerSse,
    Sse,
    SseSse,
    SseInteger,
    Memory,
}

impl ValueClass {
    pub(super) fn classify(ty: TypeRef<'_>, layout: &FfiTypeLayout) -> Self {
        if layout.size > 16 {
            return Self::Memory;
        }

        let summary = RegisterSummary::for_type(ty);
        Self::from_eightbyte_classes([
            summary.eightbyte_class(0x00ff),
            summary.eightbyte_class(0xff00),
        ])
    }

    fn from_eightbyte_classes(eightbyte_classes: [EightbyteClass; 2]) -> Self {
        match eightbyte_classes {
            [EightbyteClass::Sse, EightbyteClass::Sse] => Self::SseSse,
            [EightbyteClass::Sse, EightbyteClass::Integer] => Self::SseInteger,
            [EightbyteClass::Sse, EightbyteClass::NoClass] => Self::Sse,
            [EightbyteClass::Integer, EightbyteClass::Sse] => Self::IntegerSse,
            [EightbyteClass::Integer, EightbyteClass::Integer] => Self::IntegerInteger,
            [EightbyteClass::Integer, EightbyteClass::NoClass] => Self::Integer,
            // Nonempty aggregates have a scalar at offset zero.
            [EightbyteClass::NoClass, _] => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EightbyteClass {
    Integer,
    Sse,
    NoClass,
}
