use crate::backend::x86_64::Register;

#[derive(Debug, Default)]
struct CallFrame {
    gpr_registers: [Register; 6],
    xmm_registers: [Register; 8],
}
