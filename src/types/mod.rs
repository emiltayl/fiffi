//! Types and traits for describing FFI function signatures.
//!
//! * [`Type`] describes argument and return types. [`VariadicType`] describes the types allowed as
//!   variadic arguments.
//! * [`FfiType`] describes a Rust type's layout. Any argument to,
#![cfg_attr(
    feature = "closure",
    doc = "  or non-void return type from Rust closures used with [`Closure`](`crate::closure::Closure`) must implement [`FfiType`]."
)]
#![cfg_attr(
    not(feature = "closure"),
    doc = "  or non-void return type from Rust closures used with `Closure` must implement [`FfiType`]."
)]

extern crate alloc;

#[cfg(not(test))]
use alloc::vec::Vec;
use core::ffi::c_void;

use crate::errors::{EmptyStructError, EmptyUnionError, InvalidVariadicTypeError};

pub(crate) mod internal {
    use super::Type;
    #[cfg(not(test))]
    use super::Vec;

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct NonEmptyVec(Vec<Type>);

    impl NonEmptyVec {
        pub fn new(types: Vec<Type>) -> Option<Self> {
            if types.is_empty() {
                None
            } else {
                Some(Self(types))
            }
        }

        pub fn new_from_slice(types: &[Type]) -> Option<Self> {
            Self::new(types.to_vec())
        }

        /// # Safety
        ///
        /// * `types` must not be empty.
        pub unsafe fn new_unchecked(types: Vec<Type>) -> Self {
            Self(types)
        }

        /// # Safety
        ///
        /// * `types` must not be empty.
        pub unsafe fn new_from_slice_unchecked(types: &[Type]) -> Self {
            // SAFETY: The caller guarantees that `types` is not empty.
            unsafe { Self::new_unchecked(types.to_vec()) }
        }

        pub fn as_slice(&self) -> &[Type] {
            &self.0
        }
    }
}

/// A type description used to describe a function's arguments and return types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Type {
    /// Signed 8-bit integer
    I8,

    /// Unsigned 8-bit integer
    U8,

    /// Signed 16-bit integer
    I16,

    /// Unsigned 16-bit integer
    U16,

    /// Signed 32-bit integer
    I32,

    /// Unsigned 32-bit integer
    U32,

    /// Signed 64-bit integer
    I64,

    /// Unsigned 64-bit integer
    U64,

    /// Signed 128-bit integer
    I128,

    /// Unsigned 128-bit integer
    U128,

    /// Signed pointer-sized integer
    Isize,

    /// Unsigned pointer-sized integer
    Usize,

    /// 32-bit floating-point number
    F32,

    /// 64-bit floating-point number
    F64,

    /// An arbitrary pointer
    Pointer,

    /// C-compatible struct with at least one field.
    ///
    /// A `Type::Struct` must be created using [`Type::create_struct`] or
    /// [`Type::create_struct_from_slice`]. This ensures that the struct is not empty, as empty
    /// structs are not supported by fiffi.
    Struct(internal::NonEmptyVec),

    /// C-compatible union with at least one variant.
    ///
    /// A `Type::Union` must be created using [`Type::create_union`] or
    /// [`Type::create_union_from_slice`]. This ensures that the union is not empty, as empty
    /// unions are not supported by fiffi.
    Union(internal::NonEmptyVec),
}

/// A type description that can be used for variadic arguments.
///
/// C's default argument promotions means that 8- and 16-bit integers and 32-bit floats are not
/// valid variadic argument types for libffi. Use the promoted 32-bit integer or 64-bit float type
/// instead.
///
/// `VariadicType` implements `TryFrom<Type>` to attempt converting a [`Type`] to `VariadicType`. A
/// `VariadicType` can be converted to a [`Type`] using [`VariadicType::to_type`] or [`Type`]'s
/// `From<VariadicType>` implementation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VariadicType {
    /// Signed 32-bit integer
    I32,

    /// Unsigned 32-bit integer
    U32,

    /// Signed 64-bit integer
    I64,

    /// Unsigned 64-bit integer
    U64,

    /// Signed 128-bit integer
    I128,

    /// Unsigned 128-bit integer
    U128,

    /// Signed pointer-sized integer
    Isize,

    /// Unsigned pointer-sized integer
    Usize,

    /// 64-bit floating-point number
    F64,

    /// An arbitrary pointer
    Pointer,

    /// C-compatible struct with at least one field.
    ///
    /// A `VariadicType::Struct` must be created using [`VariadicType::create_struct`] or
    /// [`VariadicType::create_struct_from_slice`]. This ensures that the struct is not empty, as
    /// empty structs are not supported by fiffi.
    Struct(internal::NonEmptyVec),

    /// C-compatible union with at least one variant.
    ///
    /// A `VariadicType::Union` must be created using [`VariadicType::create_union`] or
    /// [`VariadicType::create_union_from_slice`]. This ensures that the union is not empty, as
    /// empty unions are not supported by fiffi.
    Union(internal::NonEmptyVec),
}

/// Size and alignment used by fiffi for a [`Type`].
///
/// This can be used to make sure that [`FfiType`] implementations are correct by verifying
/// that a Rust type and the type used by fiffi have the same memory size and alignment. Use
/// [`Type::layout`] to get the `FfiTypeLayout` for a [`Type`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FfiTypeLayout {
    /// Type alignment in bytes.
    pub align: usize,

    /// Type size in bytes.
    pub size: usize,
}

impl FfiTypeLayout {
    /// Appends a struct field and returns its offset before advancing past it.
    fn append_field(&mut self, field: Self) -> usize {
        self.size += padding_needed(self.size, field.align);
        let offset = self.size;
        self.size += field.size;
        self.align = self.align.max(field.align);
        offset
    }

    fn include_variant(&mut self, variant: Self) {
        self.align = self.align.max(variant.align);
        self.size = self.size.max(variant.size);
    }

    fn pad_to_alignment(&mut self) {
        self.size += padding_needed(self.size, self.align);
    }
}

/// A type's layout and position within a preorder traversal of a type tree.
#[derive(Debug)]
pub(crate) struct LayoutNode<'ty> {
    pub ty: &'ty Type,
    pub layout: FfiTypeLayout,
    /// Offset relative to the parent; zero for the root and union variants.
    pub offset_in_parent: usize,
    /// Exclusive end of this node's subtree, including the node itself.
    pub subtree_end: usize,
}

/// Calculate the padding needed to `size` to align with `align`.
fn padding_needed(size: usize, align: usize) -> usize {
    let remainder = size % align;
    if remainder == 0 { 0 } else { align - remainder }
}

impl Type {
    /// Creates a `Type::Struct` from member types in field order.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyStructError`] if `types` is empty.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::types::Type;
    ///
    /// #[repr(C)]
    /// struct FfiStruct(i32, f64);
    ///
    /// let ffi_struct_type = Type::create_struct(vec![Type::I32, Type::F64])?;
    ///
    /// let type_layout = ffi_struct_type.layout();
    ///
    /// assert_eq!(type_layout.align, align_of::<FfiStruct>());
    /// assert_eq!(type_layout.size, size_of::<FfiStruct>());
    ///
    /// # Ok::<(), fiffi::errors::EmptyStructError>(())
    /// ```
    pub fn create_struct(types: Vec<Type>) -> Result<Self, EmptyStructError> {
        internal::NonEmptyVec::new(types)
            .map(Self::Struct)
            .ok_or(EmptyStructError)
    }

    /// Like [`Type::create_struct`] except that it creates a `Type::Struct` from a slice rather
    /// than `Vec` of member types.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyStructError`] if `types` is empty.
    pub fn create_struct_from_slice(types: &[Type]) -> Result<Self, EmptyStructError> {
        Self::create_struct(types.to_vec())
    }

    /// Creates a `Type::Union` from its variant types.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyUnionError`] if `variants` is empty.
    pub fn create_union(variants: Vec<Type>) -> Result<Self, EmptyUnionError> {
        internal::NonEmptyVec::new(variants)
            .map(Self::Union)
            .ok_or(EmptyUnionError)
    }

    /// Like [`Type::create_union`], using a slice of variant types.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyUnionError`] if `types` is empty.
    pub fn create_union_from_slice(variants: &[Type]) -> Result<Self, EmptyUnionError> {
        Self::create_union(variants.to_vec())
    }

    /// Like [`Type::create_struct`], without checking for an empty field list.
    ///
    /// # Safety
    ///
    /// * `types` must not be empty.
    pub unsafe fn create_struct_unchecked(types: Vec<Type>) -> Self {
        // SAFETY: The caller guarantees that `types` is not empty.
        unsafe { Self::Struct(internal::NonEmptyVec::new_unchecked(types)) }
    }

    /// Like [`Type::create_struct_from_slice`], without checking for an empty field list.
    ///
    /// # Safety
    ///
    /// * `types` must not be empty.
    pub unsafe fn create_struct_from_slice_unchecked(types: &[Type]) -> Self {
        // SAFETY: The caller guarantees that `types` is not empty.
        unsafe { Self::create_struct_unchecked(types.to_vec()) }
    }

    /// Like [`Type::create_union`], without checking for an empty variant list.
    ///
    /// # Safety
    ///
    /// * `variants` must not be empty.
    pub unsafe fn create_union_unchecked(variants: Vec<Type>) -> Self {
        // SAFETY: The caller guarantees that `variants` is not empty.
        unsafe { Self::Union(internal::NonEmptyVec::new_unchecked(variants)) }
    }

    /// Like [`Type::create_union_from_slice`], without checking for an empty variant list.
    ///
    /// # Safety
    ///
    /// * `variants` must not be empty.
    pub unsafe fn create_union_from_slice_unchecked(variants: &[Type]) -> Self {
        // SAFETY: The caller guarantees that `variants` is not empty.
        unsafe { Self::create_union_unchecked(variants.to_vec()) }
    }

    /// Returns this type's size and alignment.
    pub fn layout(&self) -> FfiTypeLayout {
        match self {
            Type::I8 => FfiTypeLayout {
                align: align_of::<i8>(),
                size: size_of::<i8>(),
            },
            Type::U8 => FfiTypeLayout {
                align: align_of::<u8>(),
                size: size_of::<u8>(),
            },
            Type::I16 => FfiTypeLayout {
                align: align_of::<i16>(),
                size: size_of::<i16>(),
            },
            Type::U16 => FfiTypeLayout {
                align: align_of::<u16>(),
                size: size_of::<u16>(),
            },
            Type::I32 => FfiTypeLayout {
                align: align_of::<i32>(),
                size: size_of::<i32>(),
            },
            Type::U32 => FfiTypeLayout {
                align: align_of::<u32>(),
                size: size_of::<u32>(),
            },
            Type::I64 => FfiTypeLayout {
                align: align_of::<i64>(),
                size: size_of::<i64>(),
            },
            Type::U64 => FfiTypeLayout {
                align: align_of::<u64>(),
                size: size_of::<u64>(),
            },
            Type::I128 => FfiTypeLayout {
                align: align_of::<i128>(),
                size: size_of::<i128>(),
            },
            Type::U128 => FfiTypeLayout {
                align: align_of::<u128>(),
                size: size_of::<u128>(),
            },
            Type::Isize => FfiTypeLayout {
                align: align_of::<isize>(),
                size: size_of::<isize>(),
            },
            Type::Usize => FfiTypeLayout {
                align: align_of::<usize>(),
                size: size_of::<usize>(),
            },
            Type::F32 => FfiTypeLayout {
                align: align_of::<f32>(),
                size: size_of::<f32>(),
            },
            Type::F64 => FfiTypeLayout {
                align: align_of::<f64>(),
                size: size_of::<f64>(),
            },
            Type::Pointer => FfiTypeLayout {
                align: align_of::<*const c_void>(),
                size: size_of::<*const c_void>(),
            },
            Type::Struct(type_vec) => {
                let mut layout = FfiTypeLayout { align: 1, size: 0 };

                for field in type_vec.as_slice() {
                    layout.append_field(field.layout());
                }

                layout.pad_to_alignment();

                layout
            }
            Type::Union(type_vec) => {
                let mut layout = FfiTypeLayout { align: 1, size: 0 };

                for field in type_vec.as_slice() {
                    layout.include_variant(field.layout());
                }

                layout.pad_to_alignment();

                layout
            }
        }
    }

    /// Prepares layouts for this type and all descendants in preorder, replacing the nodes
    /// while retaining their allocation for reuse.
    pub(crate) fn layout_nodes_into<'ty>(&'ty self, nodes: &mut Vec<LayoutNode<'ty>>) {
        nodes.clear();
        self.append_layout_node(nodes);
    }

    fn append_layout_node<'ty>(&'ty self, nodes: &mut Vec<LayoutNode<'ty>>) -> usize {
        let node_index = nodes.len();
        let mut layout = FfiTypeLayout { align: 1, size: 0 };
        nodes.push(LayoutNode {
            ty: self,
            layout,
            offset_in_parent: 0,
            subtree_end: node_index + 1,
        });

        match self {
            Type::Struct(fields) => {
                for field in fields.as_slice() {
                    let child_index = field.append_layout_node(nodes);
                    let child = &mut nodes[child_index];
                    child.offset_in_parent = layout.append_field(child.layout);
                }
                layout.pad_to_alignment();
            }
            Type::Union(variants) => {
                for variant in variants.as_slice() {
                    let child_index = variant.append_layout_node(nodes);
                    layout.include_variant(nodes[child_index].layout);
                }
                layout.pad_to_alignment();
            }
            // Only scalar layout lookups are used here: aggregate layouts come from the
            // completed children, so each type node is prepared exactly once.
            _ => layout = self.layout(),
        }

        nodes[node_index].layout = layout;
        nodes[node_index].subtree_end = nodes.len();
        node_index
    }

    /// Returns struct field offsets in declaration order, or an empty vector for other types.
    pub fn field_offsets(&self) -> Vec<usize> {
        // TODO benchmark whether it is worth it to combine `Type::layout` and `Type::field_offsets`
        // for structs to avoid iterating over fields twice.
        if let Type::Struct(type_vec) = self {
            let mut offsets = Vec::with_capacity(type_vec.as_slice().len());
            let mut offset = 0;

            for field in type_vec.as_slice() {
                let field_layout = field.layout();
                offset += padding_needed(offset, field_layout.align);
                offsets.push(offset);
                offset += field_layout.size;
            }

            offsets
        } else {
            Vec::new()
        }
    }
}

impl VariadicType {
    /// Creates a `VariadicType::Struct` from member types in field order.
    ///
    /// Struct variadic arguments may contain arbitrary non-empty [`Type`] fields, including fields
    /// that are not themselves valid standalone variadic argument types.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyStructError`] if `types` is empty.
    pub fn create_struct(types: Vec<Type>) -> Result<Self, EmptyStructError> {
        internal::NonEmptyVec::new(types)
            .map(Self::Struct)
            .ok_or(EmptyStructError)
    }

    /// Like [`VariadicType::create_struct`] except that it creates a `VariadicType::Struct` from a
    /// slice rather than `Vec` of member types.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyStructError`] if `types` is empty.
    pub fn create_struct_from_slice(types: &[Type]) -> Result<Self, EmptyStructError> {
        Self::create_struct(types.to_vec())
    }

    /// Creates a `VariadicType::Union` from its variant types.
    ///
    /// Union variants may use any [`Type`], including types subject to variadic promotions.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyUnionError`] if `variants` is empty.
    pub fn create_union(variants: Vec<Type>) -> Result<Self, EmptyUnionError> {
        internal::NonEmptyVec::new(variants)
            .map(Self::Union)
            .ok_or(EmptyUnionError)
    }

    /// Like [`VariadicType::create_union`], using a slice of variant types.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyStructError`] if `variants` is empty.
    pub fn create_union_from_slice(variants: &[Type]) -> Result<Self, EmptyUnionError> {
        Self::create_union(variants.to_vec())
    }

    /// Like [`VariadicType::create_struct`], without checking for an empty field list.
    ///
    /// # Safety
    ///
    /// * `types` must not be empty.
    pub unsafe fn create_struct_unchecked(types: Vec<Type>) -> Self {
        // SAFETY: The caller guarantees that `types` is not empty.
        unsafe { Self::Struct(internal::NonEmptyVec::new_unchecked(types)) }
    }

    /// Like [`VariadicType::create_struct_from_slice`], without checking for an empty field list.
    ///
    /// # Safety
    ///
    /// * `types` must not be empty.
    pub unsafe fn create_struct_from_slice_unchecked(types: &[Type]) -> Self {
        // SAFETY: The caller guarantees that `types` is not empty.
        unsafe { Self::create_struct_unchecked(types.to_vec()) }
    }

    /// Like [`VariadicType::create_union`], without checking for an empty variant list.
    ///
    /// # Safety
    ///
    /// * `variants` must not be empty.
    pub unsafe fn create_union_unchecked(variants: Vec<Type>) -> Self {
        // SAFETY: The caller guarantees that `variants` is not empty.
        unsafe { Self::Union(internal::NonEmptyVec::new_unchecked(variants)) }
    }

    /// Like [`VariadicType::create_union_from_slice`], without checking for an empty variant list.
    ///
    /// # Safety
    ///
    /// * `variants` must not be empty.
    pub unsafe fn create_union_from_slice_unchecked(variants: &[Type]) -> Self {
        // SAFETY: The caller guarantees that `types` is not empty.
        unsafe { Self::create_union_unchecked(variants.to_vec()) }
    }

    /// Convert a `&VariadicType` to a `Type`.
    pub fn to_type(&self) -> Type {
        self.clone().into()
    }
}

impl TryFrom<Type> for VariadicType {
    type Error = InvalidVariadicTypeError;

    fn try_from(value: Type) -> Result<Self, Self::Error> {
        match value {
            Type::I32 => Ok(Self::I32),
            Type::U32 => Ok(Self::U32),
            Type::I64 => Ok(Self::I64),
            Type::U64 => Ok(Self::U64),
            Type::I128 => Ok(Self::I128),
            Type::U128 => Ok(Self::U128),
            Type::Isize => Ok(Self::Isize),
            Type::Usize => Ok(Self::Usize),
            Type::F64 => Ok(Self::F64),
            Type::Pointer => Ok(Self::Pointer),
            Type::Struct(types) => Ok(Self::Struct(types)),
            Type::Union(types) => Ok(Self::Union(types)),
            Type::I8 | Type::U8 | Type::I16 | Type::U16 | Type::F32 => {
                Err(InvalidVariadicTypeError(value))
            }
        }
    }
}

impl From<VariadicType> for Type {
    fn from(value: VariadicType) -> Self {
        match value {
            VariadicType::I32 => Self::I32,
            VariadicType::U32 => Self::U32,
            VariadicType::I64 => Self::I64,
            VariadicType::U64 => Self::U64,
            VariadicType::I128 => Self::I128,
            VariadicType::U128 => Self::U128,
            VariadicType::Isize => Self::Isize,
            VariadicType::Usize => Self::Usize,
            VariadicType::F64 => Self::F64,
            VariadicType::Pointer => Self::Pointer,
            VariadicType::Struct(types) => Self::Struct(types),
            VariadicType::Union(types) => Self::Union(types),
        }
    }
}

/// Rust types that can be described by a [`Type`] for FFI calls.
///
/// Implementors must be `Copy` because fiffi copies values when calling functions.
///
/// # Safety
///
/// * [`FfiType::ffi_type`] must describe the type's field types, layout, and ABI.
/// * Composite types must use a C-compatible representation such as `#[repr(C)]` or
///   `#[repr(transparent)]`.
/// * For `#[repr(transparent)]` types, `ffi_type()` must return the contained type's description.
///
/// # Examples
///
/// ```
/// use std::ffi::c_void;
/// use std::mem::offset_of;
///
/// use fiffi::types::{FfiType, Type};
///
/// #[derive(Clone, Copy)]
/// #[repr(C)]
/// struct Pair {
///     left: i32,
///     right: f64,
/// }
///
/// // SAFETY: `Pair` is `repr(C)`, `Copy`, and the field list matches declaration order.
/// unsafe impl FfiType for Pair {
///     fn ffi_type() -> Type {
///         // SAFETY: The vec provided to `Type::create_struct_unchecked` is not empty.
///         unsafe { Type::create_struct_unchecked(vec![Type::I32, Type::F64]) }
///     }
/// }
///
/// let pair_layout = <Pair as FfiType>::ffi_type().layout();
///
/// assert_eq!(pair_layout.align, align_of::<Pair>());
/// assert_eq!(pair_layout.size, size_of::<Pair>());
///
/// let pair_offsets = <Pair as FfiType>::ffi_type().field_offsets();
///
/// assert_eq!(pair_offsets[0], offset_of!(Pair, left));
/// assert_eq!(pair_offsets[1], offset_of!(Pair, right));
///
/// #[derive(Clone, Copy)]
/// #[repr(transparent)]
/// struct Handle(*mut c_void);
///
/// // SAFETY: `Handle` is transparent over a raw pointer.
/// unsafe impl FfiType for Handle {
///     fn ffi_type() -> Type {
///         Type::Pointer
///     }
/// }
///
/// assert_eq!(
///     <Handle as FfiType>::ffi_type().layout(),
///     Type::Pointer.layout(),
/// );
/// ```
pub unsafe trait FfiType: Copy {
    /// Returns the FFI type description for `Self`.
    ///
    /// See [`FfiType`] for examples of `ffi_type` implementations.
    fn ffi_type() -> Type;
}

// SAFETY: `i8` has the C ABI layout described by `Type::I8`.
unsafe impl FfiType for i8 {
    fn ffi_type() -> Type {
        Type::I8
    }
}

// SAFETY: `u8` has the C ABI layout described by `Type::U8`.
unsafe impl FfiType for u8 {
    fn ffi_type() -> Type {
        Type::U8
    }
}

// SAFETY: `i16` has the C ABI layout described by `Type::I16`.
unsafe impl FfiType for i16 {
    fn ffi_type() -> Type {
        Type::I16
    }
}

// SAFETY: `u16` has the C ABI layout described by `Type::U16`.
unsafe impl FfiType for u16 {
    fn ffi_type() -> Type {
        Type::U16
    }
}

// SAFETY: `i32` has the C ABI layout described by `Type::I32`.
unsafe impl FfiType for i32 {
    fn ffi_type() -> Type {
        Type::I32
    }
}

// SAFETY: `u32` has the C ABI layout described by `Type::U32`.
unsafe impl FfiType for u32 {
    fn ffi_type() -> Type {
        Type::U32
    }
}

// SAFETY: `i64` has the C ABI layout described by `Type::I64`.
unsafe impl FfiType for i64 {
    fn ffi_type() -> Type {
        Type::I64
    }
}

// SAFETY: `u64` has the C ABI layout described by `Type::U64`.
unsafe impl FfiType for u64 {
    fn ffi_type() -> Type {
        Type::U64
    }
}

// SAFETY: `i128` has the C ABI layout described by `Type::I128`.
unsafe impl FfiType for i128 {
    fn ffi_type() -> Type {
        Type::I128
    }
}

// SAFETY: `u128` has the C ABI layout described by `Type::U128`.
unsafe impl FfiType for u128 {
    fn ffi_type() -> Type {
        Type::U128
    }
}

// SAFETY: `isize` has the same layout as the target pointer-sized signed integer.
unsafe impl FfiType for isize {
    fn ffi_type() -> Type {
        Type::Isize
    }
}

// SAFETY: `usize` has the same layout as the target pointer-sized unsigned integer.
unsafe impl FfiType for usize {
    fn ffi_type() -> Type {
        Type::Usize
    }
}

// SAFETY: `f32` has the C ABI layout described by `Type::F32`.
unsafe impl FfiType for f32 {
    fn ffi_type() -> Type {
        Type::F32
    }
}

// SAFETY: `f64` has the C ABI layout described by `Type::F64`.
unsafe impl FfiType for f64 {
    fn ffi_type() -> Type {
        Type::F64
    }
}

// SAFETY: Raw const pointers have the C ABI pointer layout.
unsafe impl<T> FfiType for *const T {
    fn ffi_type() -> Type {
        Type::Pointer
    }
}

// SAFETY: Raw mut pointers have the C ABI pointer layout.
unsafe impl<T> FfiType for *mut T {
    fn ffi_type() -> Type {
        Type::Pointer
    }
}

#[cfg(test)]
mod tests {
    use core::any::type_name;
    use core::ffi::c_void;
    use core::mem::offset_of;

    use super::{FfiType, FfiTypeLayout, LayoutNode, Type};
    use crate::test_utils::structs::*;
    use crate::test_utils::unions::*;

    fn layout_nodes(ty: &Type) -> Vec<LayoutNode<'_>> {
        let mut nodes = Vec::new();
        ty.layout_nodes_into(&mut nodes);
        nodes
    }

    fn assert_ffi_layout<T: FfiType>() {
        let ty = T::ffi_type();
        let layout = ty.layout();

        assert_eq!(
            layout.align,
            align_of::<T>(),
            "alignment mismatch for {}",
            type_name::<T>(),
        );
        assert_eq!(
            layout.size,
            size_of::<T>(),
            "size mismatch for {}",
            type_name::<T>(),
        );

        let nodes = layout_nodes(&ty);
        assert_eq!(nodes[0].layout, layout, "{}", type_name::<T>());
        assert_eq!(nodes[0].offset_in_parent, 0);
        assert_eq!(nodes[0].subtree_end, nodes.len());
        assert!(core::ptr::eq(nodes[0].ty, &ty));
    }

    fn assert_field_offsets<T: FfiType>(expected: &[usize]) {
        let offsets = T::ffi_type().field_offsets();

        assert_eq!(
            offsets.as_slice(),
            expected,
            "field offset mismatch for {}",
            type_name::<T>(),
        );
    }

    macro_rules! assert_ffi_layouts {
        ($($type:ty),+ $(,)?) => {
            $(assert_ffi_layout::<$type>();)+
        };
    }

    fn expected_node<T>(offset: usize, subtree_end: usize) -> (FfiTypeLayout, usize, usize) {
        (
            FfiTypeLayout {
                size: size_of::<T>(),
                align: align_of::<T>(),
            },
            offset,
            subtree_end,
        )
    }

    fn assert_node_metadata(nodes: &[LayoutNode<'_>], expected: &[(FfiTypeLayout, usize, usize)]) {
        let actual: Vec<_> = nodes
            .iter()
            .map(|node| (node.layout, node.offset_in_parent, node.subtree_end))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn layout_nodes_include_scalar_roots() {
        let cases = [
            (Type::F64, expected_node::<f64>(0, 1)),
            (Type::U128, expected_node::<u128>(0, 1)),
            (Type::Pointer, expected_node::<*const c_void>(0, 1)),
        ];
        for (ty, expected) in cases {
            let nodes = layout_nodes(&ty);
            assert_node_metadata(&nodes, &[expected]);
            assert!(core::ptr::eq(nodes[0].ty, &ty));
        }
    }

    #[test]
    fn layout_nodes_preserve_nested_struct_padding_and_sibling_boundaries() {
        #[repr(C)]
        struct Outer {
            nested: NestedU8U32x2,
            tail: u16,
        }

        let ty = Type::create_struct(vec![NestedU8U32x2::ffi_type(), Type::U16]).unwrap();
        let nodes = layout_nodes(&ty);
        assert_node_metadata(
            &nodes,
            &[
                expected_node::<Outer>(0, 7),
                expected_node::<NestedU8U32x2>(offset_of!(Outer, nested), 6),
                expected_node::<u8>(offset_of!(NestedU8U32x2, tag), 3),
                expected_node::<U32x2>(offset_of!(NestedU8U32x2, x), 6),
                expected_node::<u32>(offset_of!(U32x2, a), 5),
                expected_node::<u32>(offset_of!(U32x2, b), 6),
                expected_node::<u16>(offset_of!(Outer, tail), 7),
            ],
        );
        assert_eq!(nodes[2].ty, &Type::U8);
        assert_eq!(nodes[4].ty, &Type::U32);
        assert_eq!(nodes[6].ty, &Type::U16);
    }

    #[test]
    fn layout_nodes_keep_union_variant_offsets_relative_to_the_union() {
        let ty = NestedU8UnionU64F64::ffi_type();
        let nodes = layout_nodes(&ty);
        assert_node_metadata(
            &nodes,
            &[
                expected_node::<NestedU8UnionU64F64>(0, 5),
                expected_node::<u8>(offset_of!(NestedU8UnionU64F64, tag), 2),
                expected_node::<UnionU64F64>(offset_of!(NestedU8UnionU64F64, x), 5),
                expected_node::<u64>(offset_of!(UnionU64F64, i), 4),
                expected_node::<f64>(offset_of!(UnionU64F64, f), 5),
            ],
        );

        let ty = UnionNestedF32x2U64::ffi_type();
        let nodes = layout_nodes(&ty);
        assert_node_metadata(
            &nodes,
            &[
                expected_node::<UnionNestedF32x2U64>(0, 6),
                expected_node::<F32x2>(offset_of!(UnionNestedF32x2U64, f), 4),
                expected_node::<f32>(offset_of!(F32x2, a), 3),
                expected_node::<f32>(offset_of!(F32x2, b), 4),
                expected_node::<U64>(offset_of!(UnionNestedF32x2U64, i), 6),
                expected_node::<u64>(offset_of!(U64, a), 6),
            ],
        );
    }

    #[test]
    fn layout_nodes_into_replaces_nodes_without_reallocating_sufficient_storage() {
        let original = Type::create_struct(vec![Type::U8; 17]).unwrap();
        let smaller = NestedU8UnionU64F64::ffi_type();
        let scalar = Type::U128;
        let mut nodes = layout_nodes(&original);
        let capacity = nodes.capacity();
        let pointer = nodes.as_ptr();
        assert_eq!(nodes.len(), 18);
        assert_eq!(nodes[0].layout.size, 17);

        for (ty, node_count) in [(&smaller, 5), (&scalar, 1), (&original, 18)] {
            ty.layout_nodes_into(&mut nodes);
            assert_eq!(nodes.len(), node_count);
            assert_eq!(nodes.capacity(), capacity);
            assert_eq!(nodes.as_ptr(), pointer);
            assert!(core::ptr::eq(nodes[0].ty, ty));
            assert_eq!(nodes[0].layout, ty.layout());
            assert_eq!(nodes[0].offset_in_parent, 0);
            assert_eq!(nodes[0].subtree_end, node_count);
        }
    }

    #[test]
    #[rustfmt::skip]
    fn aggregate_fixture_ffi_layouts_match_rust_layouts() {
        assert_ffi_layouts!(
            i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64, *const c_void,
            U8, U8x3, U16x3, U32x2, U32x3, U32x4, U64, U64x2, U64x3, U64x4, U128, U128x2, F32,
            F32x2, F32x3, F32x4, F64, F64x2, F64x3, F64x4, F64x8, U64F64, F64U64, U32F32, F32x3U32,
            U32F32x3, F64F32, U8U16, U8U64, U64U8, U8F64, U8F64U8, U32U64U32, U8U128, U128U8,
            U8U128U8, NestedU8U32x2, NestedF32x2x2, NestedF64x2x2, NestedU8U64x2,
            NestedUnionU32F32, NestedUnionU32F32x2, NestedU8UnionU64F64, NestedUnionU8U128U8,
            NestedU8UnionU128U8, UsizePointer, UnionI32U32, UnionI64U64, UnionU128, UnionU8U128,
            UnionU128U8, UnionU32F32, UnionU64F64, UnionNestedU8x3U64,
            UnionNestedU8x3F32x2, UnionNestedU16x3F64x2, UnionNestedU64x2,
            UnionNestedF64x2, UnionNestedU8U16U64, UnionNestedU64F64, UnionNestedF32x4U32x4,
            UnionNestedF64x2U64x2, UnionNestedF32x2U64, UnionNestedF64x4U64x4,
            UnionNestedU64x4F64x4,
        );
    }

    #[test]
    #[rustfmt::skip]
    fn aggregate_fixture_field_offsets_match_rust_offsets() {
        assert_field_offsets::<U8>(&[offset_of!(U8, a)]);
        assert_field_offsets::<U8x3>(&[offset_of!(U8x3, a), offset_of!(U8x3, b), offset_of!(U8x3, c)]);
        assert_field_offsets::<U16x3>(&[offset_of!(U16x3, a), offset_of!(U16x3, b), offset_of!(U16x3, c)]);
        assert_field_offsets::<U32x2>(&[offset_of!(U32x2, a), offset_of!(U32x2, b)]);
        assert_field_offsets::<U32x3>(&[offset_of!(U32x3, a), offset_of!(U32x3, b), offset_of!(U32x3, c)]);
        assert_field_offsets::<U32x4>(&[offset_of!(U32x4, a), offset_of!(U32x4, b), offset_of!(U32x4, c), offset_of!(U32x4, d)]);
        assert_field_offsets::<U64>(&[offset_of!(U64, a)]);
        assert_field_offsets::<U64x2>(&[offset_of!(U64x2, a), offset_of!(U64x2, b)]);
        assert_field_offsets::<U64x3>(&[offset_of!(U64x3, a), offset_of!(U64x3, b), offset_of!(U64x3, c)]);
        assert_field_offsets::<U64x4>(&[offset_of!(U64x4, a), offset_of!(U64x4, b), offset_of!(U64x4, c), offset_of!(U64x4, d)]);
        assert_field_offsets::<U128>(&[offset_of!(U128, a)]);
        assert_field_offsets::<U128x2>(&[offset_of!(U128x2, a), offset_of!(U128x2, b)]);
        assert_field_offsets::<F32>(&[offset_of!(F32, a)]);
        assert_field_offsets::<F32x2>(&[offset_of!(F32x2, a), offset_of!(F32x2, b)]);
        assert_field_offsets::<F32x3>(&[offset_of!(F32x3, a), offset_of!(F32x3, b), offset_of!(F32x3, c)]);
        assert_field_offsets::<F32x4>(&[offset_of!(F32x4, a), offset_of!(F32x4, b), offset_of!(F32x4, c), offset_of!(F32x4, d)]);
        assert_field_offsets::<F64>(&[offset_of!(F64, a)]);
        assert_field_offsets::<F64x2>(&[offset_of!(F64x2, a), offset_of!(F64x2, b)]);
        assert_field_offsets::<F64x3>(&[offset_of!(F64x3, a), offset_of!(F64x3, b), offset_of!(F64x3, c)]);
        assert_field_offsets::<F64x4>(&[offset_of!(F64x4, a), offset_of!(F64x4, b), offset_of!(F64x4, c), offset_of!(F64x4, d)]);
        assert_field_offsets::<F64x8>(&[
            offset_of!(F64x8, a), offset_of!(F64x8, b), offset_of!(F64x8, c), offset_of!(F64x8, d),
            offset_of!(F64x8, e), offset_of!(F64x8, f), offset_of!(F64x8, g), offset_of!(F64x8, h),
        ]);
        assert_field_offsets::<U64F64>(&[offset_of!(U64F64, a), offset_of!(U64F64, b)]);
        assert_field_offsets::<F64U64>(&[offset_of!(F64U64, a), offset_of!(F64U64, b)]);
        assert_field_offsets::<U32F32>(&[offset_of!(U32F32, a), offset_of!(U32F32, b)]);
        assert_field_offsets::<F32x3U32>(&[offset_of!(F32x3U32, a), offset_of!(F32x3U32, b), offset_of!(F32x3U32, c), offset_of!(F32x3U32, d)]);
        assert_field_offsets::<U32F32x3>(&[offset_of!(U32F32x3, a), offset_of!(U32F32x3, b), offset_of!(U32F32x3, c), offset_of!(U32F32x3, d)]);
        assert_field_offsets::<F64F32>(&[offset_of!(F64F32, a), offset_of!(F64F32, b)]);
        assert_field_offsets::<U8U16>(&[offset_of!(U8U16, a), offset_of!(U8U16, b)]);
        assert_field_offsets::<U8U64>(&[offset_of!(U8U64, a), offset_of!(U8U64, b)]);
        assert_field_offsets::<U64U8>(&[offset_of!(U64U8, a), offset_of!(U64U8, b)]);
        assert_field_offsets::<U8F64>(&[offset_of!(U8F64, a), offset_of!(U8F64, b)]);
        assert_field_offsets::<U8F64U8>(&[ offset_of!(U8F64U8, a), offset_of!(U8F64U8, b), offset_of!(U8F64U8, c)]);
        assert_field_offsets::<U32U64U32>(&[offset_of!(U32U64U32, a), offset_of!(U32U64U32, b), offset_of!(U32U64U32, c)]);
        assert_field_offsets::<U8U128>(&[offset_of!(U8U128, a), offset_of!(U8U128, b)]);
        assert_field_offsets::<U128U8>(&[offset_of!(U128U8, a), offset_of!(U128U8, b)]);
        assert_field_offsets::<U8U128U8>(&[offset_of!(U8U128U8, a), offset_of!(U8U128U8, b), offset_of!(U8U128U8, c)]);
        assert_field_offsets::<NestedU8U32x2>(&[offset_of!(NestedU8U32x2, tag), offset_of!(NestedU8U32x2, x)]);
        assert_field_offsets::<NestedF32x2x2>(&[offset_of!(NestedF32x2x2, x), offset_of!(NestedF32x2x2, y)]);
        assert_field_offsets::<NestedF64x2x2>(&[offset_of!(NestedF64x2x2, x), offset_of!(NestedF64x2x2, y)]);
        assert_field_offsets::<NestedU8U64x2>(&[offset_of!(NestedU8U64x2, tag), offset_of!(NestedU8U64x2, x)]);
        assert_field_offsets::<NestedUnionU32F32>(&[offset_of!(NestedUnionU32F32, x)]);
        assert_field_offsets::<NestedUnionU32F32x2>(&[offset_of!(NestedUnionU32F32x2, x), offset_of!(NestedUnionU32F32x2, y)]);
        assert_field_offsets::<NestedU8UnionU64F64>(&[offset_of!(NestedU8UnionU64F64, tag), offset_of!(NestedU8UnionU64F64, x)]);
        assert_field_offsets::<NestedUnionU8U128U8>(&[offset_of!(NestedUnionU8U128U8, x), offset_of!(NestedUnionU8U128U8, tail)]);
        assert_field_offsets::<NestedU8UnionU128U8>(&[offset_of!(NestedU8UnionU128U8, tag), offset_of!(NestedU8UnionU128U8, x)]);
        assert_field_offsets::<UsizePointer>(&[offset_of!(UsizePointer, size), offset_of!(UsizePointer, pointer)]);
    }
}
