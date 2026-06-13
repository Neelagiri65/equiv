//! Static WASM module analysis: enough structure to enforce assume-flags
//! before any execution (spec §3 assume-flags; HANDOFF gap #3).

use wasmparser::{Operator, Parser, Payload};

#[derive(Debug, Default, Clone)]
pub struct ModuleFacts {
    pub func_imports: usize,
    pub has_call_indirect: bool,
    pub has_memory_grow: bool,
    pub has_float_ops: bool,
    pub exported_funcs: Vec<String>,
}

#[derive(Debug)]
pub enum WasmError {
    Parse(String),
}

pub fn analyze(bytes: &[u8]) -> Result<ModuleFacts, WasmError> {
    let mut facts = ModuleFacts::default();
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| WasmError::Parse(e.to_string()))?;
        match payload {
            Payload::ImportSection(reader) => {
                for imp in reader {
                    let imp = imp.map_err(|e| WasmError::Parse(e.to_string()))?;
                    if matches!(imp.ty, wasmparser::TypeRef::Func(_)) {
                        facts.func_imports += 1;
                    }
                }
            }
            Payload::ExportSection(reader) => {
                for ex in reader {
                    let ex = ex.map_err(|e| WasmError::Parse(e.to_string()))?;
                    if ex.kind == wasmparser::ExternalKind::Func {
                        facts.exported_funcs.push(ex.name.to_string());
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut reader = body
                    .get_operators_reader()
                    .map_err(|e| WasmError::Parse(e.to_string()))?;
                while !reader.eof() {
                    let op = reader.read().map_err(|e| WasmError::Parse(e.to_string()))?;
                    match op {
                        Operator::CallIndirect { .. } => facts.has_call_indirect = true,
                        Operator::MemoryGrow { .. } => facts.has_memory_grow = true,
                        _ => {
                            // Float routing (spec §6.3): any f32/f64 opcode.
                            let name = format!("{op:?}");
                            if name.starts_with("F32") || name.starts_with("F64") {
                                facts.has_float_ops = true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(facts)
}

/// Which assume-flags the module statically violates.
pub fn violated_flags(
    facts: &ModuleFacts,
    flags: &[equiv_core::ast::AssumeFlag],
) -> Vec<equiv_core::ast::AssumeFlag> {
    use equiv_core::ast::AssumeFlag as F;
    flags
        .iter()
        .copied()
        .filter(|f| match f {
            F::NoImports => facts.func_imports > 0,
            F::NoIndirect => facts.has_call_indirect,
            F::NoMemoryGrow => facts.has_memory_grow,
            // NoTrap and intrinsics flags are dynamic/engine concerns.
            _ => false,
        })
        .collect()
}
