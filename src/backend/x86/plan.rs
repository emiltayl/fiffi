extern crate alloc;

#[cfg(not(test))]
use alloc::{boxed::Box, vec::Vec};

use super::Abi;
use super::classification::{FloatKind, ReturnClass};
use crate::backend::CallSignature;
use crate::types::{FfiTypeLayout, ScalarType, TypeRef};

fn reserve_stack(size: &mut usize, value_size: usize, alignment: usize) -> usize {
    let offset = size
        .checked_next_multiple_of(alignment)
        .expect("x86 stack alignment overflow");
    *size = offset
        .strict_add(value_size)
        .checked_next_multiple_of(4)
        .expect("x86 stack size overflow");
    offset
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ArgumentDestination {
    Stack(usize),
    /// Register(0) is ECX and Register(1) is EDX.
    Register(usize),
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub(super) enum ArgumentMoveKind {
    Copy,
    SignExtend,
    ZeroExtend,
    StackAddress,
    ReturnPointer,
}

#[derive(Clone, Debug)]
pub(super) struct ArgumentMove {
    /// Argument index, or offset from pre-call ESP for StackAddress/ReturnPointer.
    pub(super) source: usize,
    /// Stack byte offset or register index (zero is ECX, one is EDX), selected by kind.
    pub(super) destination: usize,
    pub(super) size: usize,
    pub(super) source_offset: u8,
    pub(super) kind: ArgumentMoveKind,
}

impl ArgumentMove {
    fn argument(
        source: usize,
        source_offset: usize,
        destination: ArgumentDestination,
        size: usize,
        ty: TypeRef<'_>,
        layout: FfiTypeLayout,
        stack_end: usize,
    ) -> Self {
        debug_assert!(source_offset.strict_add(size) <= layout.size);

        let kind = match ty {
            TypeRef::Scalar(ScalarType::I8 | ScalarType::I16) => ArgumentMoveKind::SignExtend,
            TypeRef::Scalar(ScalarType::U8 | ScalarType::U16) => ArgumentMoveKind::ZeroExtend,
            _ => ArgumentMoveKind::Copy,
        };

        let write_size = match kind {
            ArgumentMoveKind::Copy => size,
            ArgumentMoveKind::SignExtend | ArgumentMoveKind::ZeroExtend => {
                debug_assert!(source_offset == 0 && matches!(size, 1 | 2));
                4
            }
            ArgumentMoveKind::StackAddress | ArgumentMoveKind::ReturnPointer => {
                unreachable!("pointer moves have no argument source")
            }
        };

        let destination = match destination {
            ArgumentDestination::Register(index) => {
                // Register copies are always one full word; narrow integers use extension.
                debug_assert!(index < 2 && write_size == 4);
                index
            }
            ArgumentDestination::Stack(offset) => {
                // The reservation includes slot rounding for narrow integer extensions.
                debug_assert!(offset.strict_add(write_size) <= stack_end);
                offset
            }
        };

        let source_offset =
            u8::try_from(source_offset).expect("x86 argument source offset exceeds u8");

        Self {
            source,
            destination,
            size,
            source_offset,
            kind,
        }
    }

    fn stack_address(source: usize, layout: FfiTypeLayout, stack_end: usize) -> Self {
        debug_assert!(layout.align <= 16 && source.is_multiple_of(layout.align));
        debug_assert!(source.strict_add(layout.size) <= stack_end);

        Self {
            source,
            destination: 0,
            size: 4,
            source_offset: 0,
            kind: ArgumentMoveKind::StackAddress,
        }
    }

    fn return_pointer(
        source: usize,
        destination: ArgumentDestination,
        layout: FfiTypeLayout,
        stack_end: usize,
    ) -> Self {
        debug_assert!(layout.align <= 16 && source.is_multiple_of(layout.align));
        debug_assert!(source.strict_add(layout.size) <= stack_end);

        let destination = match destination {
            ArgumentDestination::Stack(offset) => {
                debug_assert!(offset.strict_add(4) <= source);
                offset
            }
            ArgumentDestination::Register(index) => {
                debug_assert!(index < 2);
                index
            }
        };

        Self {
            source,
            destination,
            size: 4,
            source_offset: 0,
            kind: ArgumentMoveKind::ReturnPointer,
        }
    }
}

fn push_move(
    arguments: &mut Vec<ArgumentMove>,
    registers: &mut [Option<ArgumentMove>; 2],
    destination: ArgumentDestination,
    operation: ArgumentMove,
) {
    match destination {
        ArgumentDestination::Stack(offset) => {
            debug_assert_eq!(operation.destination, offset);
            debug_assert!(!matches!(operation.kind, ArgumentMoveKind::StackAddress));

            arguments.push(operation);
        }

        ArgumentDestination::Register(index) => {
            debug_assert_eq!(operation.destination, index);
            debug_assert!(operation.size <= 4);
            let previous = registers[index].replace(operation);
            debug_assert!(previous.is_none(), "duplicate x86 register destination");
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ReturnStrategy {
    Void,
    Integer(usize),
    X87(FloatKind),
    Indirect { fallback_stack_size: usize },
}

#[derive(Clone, Debug)]
pub(super) struct MarshalPlan {
    pub(super) arguments: Box<[ArgumentMove]>,
    /// Stack destinations precede at most two register destinations in `arguments`.
    pub(super) stack_move_count: usize,
    pub(super) stack_size: usize,
    pub(super) return_strategy: ReturnStrategy,
}

impl MarshalPlan {
    pub(super) fn build(signature: CallSignature<'_>, abi: Abi) -> Self {
        let result = ReturnClass::classify(signature.return_type());
        let mut stack_size = 0;
        let mut next_register = 0;

        // MinGW (also UEFI's LLVM target) places thiscall sret in ECX. Other targets stack sret,
        // leaving ECX available for explicit arguments. Fastcall's hidden pointer is never marked
        // inreg and does not consume an argument register.
        let hidden_pointer = if matches!(result, ReturnClass::Indirect(_)) {
            if abi == Abi::Thiscall
                && cfg!(any(
                    all(windows, not(target_env = "msvc")),
                    target_os = "uefi"
                ))
            {
                next_register = 1;
                ArgumentDestination::Register(0)
            } else {
                ArgumentDestination::Stack(reserve_stack(&mut stack_size, 4, 4))
            }
        } else {
            ArgumentDestination::Stack(0)
        };

        let mut arguments = Vec::with_capacity(signature.argument_count());
        let mut registers = [None, None];
        let mut deferred_thiscall_aggregate: Option<(usize, TypeRef<'_>, FfiTypeLayout)> = None;

        for (index, ty) in signature.arguments().enumerate() {
            let layout = ty.layout();

            if abi == Abi::Thiscall
                && next_register == 0
                // UEFI soft-float arguments can be passed in integer registers.
                && (cfg!(target_os = "uefi")
                    || !matches!(ty, TypeRef::Scalar(ScalarType::F32 | ScalarType::F64)))
            {
                next_register = 1;
                // LLVM assigns thiscall registers before byval handling, so an aggregate's lowered
                // pointer can occupy ECX. Its copy must follow all stack arguments.
                if matches!(ty, TypeRef::Struct(_) | TypeRef::Union(_)) {
                    deferred_thiscall_aggregate = Some((index, ty, layout));
                } else {
                    place_thiscall_argument(
                        &mut arguments,
                        &mut registers,
                        index,
                        layout,
                        ty,
                        &mut stack_size,
                    );
                }

                continue;
            }

            let register = if abi == Abi::Fastcall {
                fastcall_register(ty, &mut next_register)
            } else {
                None
            };

            let destination = register.map_or_else(
                || {
                    // Ordinary aggregates use byval alignment four on both classifiers. Native
                    // alignment (including `u128` members) does not change this. Rustc's vector and
                    // explicit `repr(align)` exceptions are not supported.
                    let alignment =
                        if matches!(ty, TypeRef::Scalar(ScalarType::I128 | ScalarType::U128))
                            && matches!(abi, Abi::SysV | Abi::Cdecl | Abi::Stdcall)
                        {
                            16
                        } else {
                            4
                        };

                    ArgumentDestination::Stack(reserve_stack(
                        &mut stack_size,
                        layout.size,
                        alignment,
                    ))
                },
                ArgumentDestination::Register,
            );

            let operation =
                ArgumentMove::argument(index, 0, destination, layout.size, ty, layout, stack_size);
            push_move(&mut arguments, &mut registers, destination, operation);
        }

        let argument_stack_end = stack_size;

        if let Some((index, ty, layout)) = deferred_thiscall_aggregate {
            let copy_offset = reserve_stack(&mut stack_size, layout.size, layout.align);

            debug_assert!(copy_offset >= argument_stack_end);
            debug_assert_eq!(copy_offset % layout.align, 0);

            arguments.push(ArgumentMove::argument(
                index,
                0,
                ArgumentDestination::Stack(copy_offset),
                layout.size,
                ty,
                layout,
                stack_size,
            ));

            // The stack prefix copies the value before its address is loaded into ECX.
            push_move(
                &mut arguments,
                &mut registers,
                ArgumentDestination::Register(0),
                ArgumentMove::stack_address(copy_offset, layout, stack_size),
            );
        }

        let return_strategy = match result {
            ReturnClass::Void => ReturnStrategy::Void,
            ReturnClass::Integer(size) => ReturnStrategy::Integer(size),
            ReturnClass::X87(kind) => ReturnStrategy::X87(kind),
            ReturnClass::Indirect(layout) => {
                let mut fallback_stack_size = stack_size;
                let fallback_offset =
                    reserve_stack(&mut fallback_stack_size, layout.size, layout.align);

                push_move(
                    &mut arguments,
                    &mut registers,
                    hidden_pointer,
                    ArgumentMove::return_pointer(
                        fallback_offset,
                        hidden_pointer,
                        layout,
                        fallback_stack_size,
                    ),
                );

                ReturnStrategy::Indirect {
                    fallback_stack_size,
                }
            }
        };

        let stack_move_count = arguments.len();
        arguments.extend(registers.into_iter().flatten());

        // Assembly aligns every call to 16, which also satisfies all supported value alignments. It
        // checks address subtraction and probes the whole allocation.
        Self {
            arguments: arguments.into_boxed_slice(),
            stack_move_count,
            stack_size,
            return_strategy,
        }
    }
}

fn place_thiscall_argument(
    arguments: &mut Vec<ArgumentMove>,
    registers: &mut [Option<ArgumentMove>; 2],
    index: usize,
    layout: FfiTypeLayout,
    ty: TypeRef<'_>,
    stack_size: &mut usize,
) {
    let size = layout.size.min(4);

    let operation = ArgumentMove::argument(
        index,
        0,
        ArgumentDestination::Register(0),
        size,
        ty,
        layout,
        *stack_size,
    );

    push_move(
        arguments,
        registers,
        ArgumentDestination::Register(0),
        operation,
    );

    if layout.size > size {
        // For thiscall, integers wider than 32 bits have their least significant 32 bits passed in
        // `ecx` if it is available and the rest on contiguous stack slots.
        let remaining = layout.size - size;
        let offset = reserve_stack(stack_size, remaining, 4);
        arguments.push(ArgumentMove::argument(
            index,
            size,
            ArgumentDestination::Stack(offset),
            remaining,
            ty,
            layout,
            *stack_size,
        ));
    }
}

fn fastcall_register(ty: TypeRef<'_>, next: &mut usize) -> Option<usize> {
    match ty {
        TypeRef::Struct(_)
        | TypeRef::Union(_)
        | TypeRef::Scalar(ScalarType::F32 | ScalarType::F64) => None,

        TypeRef::Scalar(
            ScalarType::I64 | ScalarType::U64 | ScalarType::I128 | ScalarType::U128,
        ) => {
            // For fastcall, 64 and 128 bit integers are not passed in registers, but still consume
            // the register slots.
            *next = 2;
            None
        }

        TypeRef::Scalar(_) if *next < 2 => {
            let register = *next;
            *next += 1;
            Some(register)
        }

        TypeRef::Scalar(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::reserve_stack;

    #[test]
    fn stack_allocation_overflow() {
        // These boundaries cannot be exercised by calls without enormous allocations.
        for (mut size, value_size, alignment) in [
            (usize::MAX, 4, 16),    // alignment
            (usize::MAX - 3, 4, 4), // addition
            (0, usize::MAX, 4),     // slot rounding
        ] {
            assert!(
                std::panic::catch_unwind(move || reserve_stack(&mut size, value_size, alignment))
                    .is_err()
            );
        }
    }
}
