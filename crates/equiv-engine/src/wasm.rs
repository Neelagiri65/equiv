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
    /// Signature of each exported function, in export order (same order as
    /// `exported_funcs`). `None` where a type is outside the five value
    /// types a contract can name.
    pub exported_sigs: Vec<Option<FuncSig>>,
    /// Linear memories the module has, imported or defined.
    pub memories: usize,
    /// Whether a memory is exported (under any name), so its bytes can be
    /// read from outside.
    pub memory_exported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncSig {
    pub params: Vec<equiv_core::ast::ValType>,
    pub results: Vec<equiv_core::ast::ValType>,
}

impl ModuleFacts {
    /// Signature of the exported function `name`: outer `None` if there is
    /// no such export, inner `None` if its types cannot be named.
    pub fn export_sig(&self, name: &str) -> Option<Option<&FuncSig>> {
        let i = self.exported_funcs.iter().position(|f| f == name)?;
        Some(self.exported_sigs.get(i).and_then(|s| s.as_ref()))
    }
}

fn core_type(t: wasmparser::ValType) -> Option<equiv_core::ast::ValType> {
    use equiv_core::ast::ValType as V;
    Some(match t {
        wasmparser::ValType::I32 => V::I32,
        wasmparser::ValType::I64 => V::I64,
        wasmparser::ValType::F32 => V::F32,
        wasmparser::ValType::F64 => V::F64,
        wasmparser::ValType::V128 => V::V128,
        wasmparser::ValType::Ref(_) => return None,
    })
}

/// How a contract writes a signature, for messages.
pub fn sig_text(params: &[equiv_core::ast::ValType], results: &[equiv_core::ast::ValType]) -> String {
    use equiv_core::ast::ValType as V;
    let name = |t: &V| match t {
        V::I32 => "i32",
        V::I64 => "i64",
        V::F32 => "f32",
        V::F64 => "f64",
        V::V128 => "v128",
    };
    let list = |ts: &[V]| ts.iter().map(name).collect::<Vec<_>>().join(" ");
    format!("(param {}) (result {})", list(params), list(results))
        .replace("(param )", "(param)")
        .replace("(result )", "(result)")
}

#[derive(Debug)]
pub enum WasmError {
    Parse(String),
}

pub fn analyze(bytes: &[u8]) -> Result<ModuleFacts, WasmError> {
    let mut facts = ModuleFacts::default();
    // Function index space: type index of every function, imports first.
    let mut types: Vec<Option<FuncSig>> = Vec::new();
    let mut func_types: Vec<u32> = Vec::new();
    let mut func_exports: Vec<u32> = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| WasmError::Parse(e.to_string()))?;
        match payload {
            Payload::ImportSection(reader) => {
                for imp in reader {
                    let imp = imp.map_err(|e| WasmError::Parse(e.to_string()))?;
                    match imp.ty {
                        wasmparser::TypeRef::Func(ti) => {
                            facts.func_imports += 1;
                            func_types.push(ti);
                        }
                        wasmparser::TypeRef::Memory(_) => facts.memories += 1,
                        _ => {}
                    }
                }
            }
            Payload::TypeSection(reader) => {
                for group in reader {
                    let group = group.map_err(|e| WasmError::Parse(e.to_string()))?;
                    for st in group.types() {
                        types.push(match &st.composite_type.inner {
                            wasmparser::CompositeInnerType::Func(ft) => {
                                let conv = |ts: &[wasmparser::ValType]| {
                                    ts.iter().map(|t| core_type(*t)).collect::<Option<Vec<_>>>()
                                };
                                match (conv(ft.params()), conv(ft.results())) {
                                    (Some(params), Some(results)) => Some(FuncSig { params, results }),
                                    _ => None,
                                }
                            }
                            _ => None,
                        });
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ti in reader {
                    func_types.push(ti.map_err(|e| WasmError::Parse(e.to_string()))?);
                }
            }
            Payload::MemorySection(reader) => {
                for m in reader {
                    m.map_err(|e| WasmError::Parse(e.to_string()))?;
                    facts.memories += 1;
                }
            }
            Payload::ExportSection(reader) => {
                for ex in reader {
                    let ex = ex.map_err(|e| WasmError::Parse(e.to_string()))?;
                    match ex.kind {
                        wasmparser::ExternalKind::Func => {
                            facts.exported_funcs.push(ex.name.to_string());
                            func_exports.push(ex.index);
                        }
                        wasmparser::ExternalKind::Memory => facts.memory_exported = true,
                        _ => {}
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
    facts.exported_sigs = func_exports
        .iter()
        .map(|fi| {
            let ti = *func_types.get(*fi as usize)?;
            types.get(ti as usize)?.clone()
        })
        .collect();
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
