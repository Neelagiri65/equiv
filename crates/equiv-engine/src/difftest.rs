//! Deterministic differential testing: seeded input generation, execution in
//! an embedded interpreter (wasmi — pure Rust, keeps the single-static-binary
//! constraint), contract evaluation per case, frame checking. Produces
//! `tested-N` / `counterexample` verdicts — never `proved` (AC-3).

use crate::eval::{eval_bool, Env};
use equiv_core::ast::*;
use equiv_core::verdict::Verdict;
use equiv_core::UnknownReason;

/// xorshift64* — deterministic, dependency-free. The seed is recorded in the
/// receipt; test vectors are reproducible everywhere (AC-1: no host
/// randomness anywhere in a decision path).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

pub struct DiffConfig {
    pub n_cases: u64,
    pub seed: u64,
    /// Candidate samples before giving up on satisfying the pre.
    pub max_attempts: u64,
}

impl Default for DiffConfig {
    fn default() -> Self {
        DiffConfig {
            n_cases: 64,
            seed: 0x6571_7569_7600_0001, // "equiv", versioned; fixed default
            max_attempts: 64 * 50,
        }
    }
}

/// Sample one argument value. Mixed distribution: small values, memory
/// offsets, and full-width randoms — pointer-shaped args must land inside
/// linear memory often enough for rejection sampling to work.
fn sample_arg(rng: &mut Rng, ty: ValType, mem_len: usize) -> u64 {
    let r = rng.next();
    match ty {
        ValType::I32 => {
            let v = match r % 4 {
                0 => r % 256,                          // small scalar
                1 | 2 => r % (mem_len.max(1) as u64),  // in-memory offset
                _ => r & 0xffff_ffff,                  // full width
            };
            v & 0xffff_ffff
        }
        ValType::I64 => match r % 3 {
            0 => r % 256,
            1 => r % (mem_len.max(1) as u64),
            _ => r,
        },
        // Float args force float routing before we get here.
        _ => r,
    }
}

struct Execution {
    result: Option<u64>,
    new_mem: Vec<u8>,
    trapped: bool,
}

/// Instantiate fresh, seed memory, call the target once.
fn run_case(
    wasm: &[u8],
    target: &Target,
    args: &[u64],
    mem_image: &[u8],
) -> Result<Execution, String> {
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, wasm).map_err(|e| e.to_string())?;
    let mut store = wasmi::Store::new(&engine, ());
    let linker: wasmi::Linker<()> = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| e.to_string())?
        .start(&mut store)
        .map_err(|e| e.to_string())?;
    let func = instance
        .get_func(&store, &target.name)
        .ok_or_else(|| format!("export `{}` not found", target.name))?;

    let memory = instance.get_memory(&store, "memory");
    if let Some(m) = memory {
        let data = m.data_mut(&mut store);
        let n = data.len().min(mem_image.len());
        data[..n].copy_from_slice(&mem_image[..n]);
    }

    let params: Vec<wasmi::Value> = args
        .iter()
        .zip(&target.params)
        .map(|(v, t)| match t {
            ValType::I64 => wasmi::Value::I64(*v as i64),
            _ => wasmi::Value::I32(*v as u32 as i32),
        })
        .collect();
    let mut results = vec![wasmi::Value::I32(0); target.results.len()];
    let call = func.call(&mut store, &params, &mut results);

    let new_mem = match memory {
        Some(m) => m.data(&store).to_vec(),
        None => Vec::new(),
    };
    match call {
        Err(_) => Ok(Execution {
            result: None,
            new_mem,
            trapped: true,
        }),
        Ok(()) => {
            let result = results.first().map(|v| match v {
                wasmi::Value::I32(n) => *n as u32 as u64,
                wasmi::Value::I64(n) => *n as u64,
                _ => 0,
            });
            Ok(Execution {
                result,
                new_mem,
                trapped: false,
            })
        }
    }
}

/// Frame check: every byte outside the declared write regions is unchanged.
fn frame_ok(frame: &[Expr], env: &Env) -> Result<bool, crate::eval::EvalError> {
    if env.old_mem.len() != env.new_mem.len() {
        return Ok(false); // memory grew; v1 contracts assume no-memory-grow
    }
    let mut allowed = vec![false; env.old_mem.len()];
    for region in frame {
        if let crate::eval::CVal::Region(b, l) = crate::eval::eval(region, env)? {
            let end = (b as u64 + l as u64).min(env.old_mem.len() as u64);
            for slot in allowed
                .iter_mut()
                .take(end as usize)
                .skip((b as usize).min(env.old_mem.len()))
            {
                *slot = true;
            }
        }
    }
    Ok(env
        .old_mem
        .iter()
        .zip(env.new_mem)
        .zip(&allowed)
        .all(|((o, n), a)| *a || o == n))
}

/// Concretely replay solver-model arguments against the artifact (AC-4):
/// returns Some(true) iff the model genuinely violates the contract
/// (pre holds, and the post fails or the call traps). Scalar contracts only
/// — memory left at module defaults.
pub fn replay_scalar(wasm: &[u8], contract: &Contract, args: &[u64]) -> Option<bool> {
    let target = &contract.target;
    let probe = run_case(wasm, target, args, &[]).ok()?;
    let mem = probe.new_mem;
    let pre_env = Env {
        args,
        arg_types: &target.params,
        result: None,
        result_type: None,
        old_mem: &mem,
        new_mem: &mem,
        idx: None,
    };
    if !eval_bool(&contract.pre, &pre_env).ok()? {
        return Some(false); // model does not even satisfy the pre
    }
    let exec = run_case(wasm, target, args, &mem).ok()?;
    if exec.trapped {
        return Some(true);
    }
    let env = Env {
        args,
        arg_types: &target.params,
        result: exec.result,
        result_type: target.results.first().copied(),
        old_mem: &mem,
        new_mem: &exec.new_mem,
        idx: None,
    };
    Some(!eval_bool(&contract.post, &env).ok()?)
}

/// Run the differential path. The wasm module must already have passed static
/// flag checks; float routing happens in the caller.
pub fn difftest(wasm: &[u8], contract: &Contract, cfg: &DiffConfig) -> Verdict {
    let mut rng = Rng(cfg.seed);
    let target = &contract.target;

    // Determine memory size from a probe instantiation.
    let probe = match run_case(wasm, target, &vec![0; target.params.len()], &[]) {
        Ok(e) => e,
        Err(_) => return Verdict::Unknown(UnknownReason::IllFormedExpr),
    };
    let mem_len = probe.new_mem.len();

    let mut passed = 0u64;
    let mut attempts = 0u64;
    while passed < cfg.n_cases && attempts < cfg.max_attempts {
        attempts += 1;
        let args: Vec<u64> = target
            .params
            .iter()
            .map(|t| sample_arg(&mut rng, *t, mem_len))
            .collect();
        let mut mem_image = vec![0u8; mem_len];
        for chunk in mem_image.chunks_mut(8) {
            let bytes = rng.next().to_le_bytes();
            let n = chunk.len();
            chunk.copy_from_slice(&bytes[..n]);
        }

        // Pre-state env (no result, new == old) to evaluate the pre.
        let pre_env = Env {
            args: &args,
            arg_types: &target.params,
            result: None,
            result_type: None,
            old_mem: &mem_image,
            new_mem: &mem_image,
            idx: None,
        };
        match eval_bool(&contract.pre, &pre_env) {
            Ok(true) => {}
            Ok(false) => continue,              // pre unsatisfied: skip case
            Err(_) => continue,                 // pre not evaluable here: skip
        }

        let exec = match run_case(wasm, target, &args, &mem_image) {
            Ok(e) => e,
            Err(_) => return Verdict::Unknown(UnknownReason::IllFormedExpr),
        };

        // Trap under a satisfied pre is a violation (spec §3: no-trap is the
        // default obligation).
        if exec.trapped {
            return Verdict::Counterexample { args, trap: true };
        }

        let env = Env {
            args: &args,
            arg_types: &target.params,
            result: exec.result,
            result_type: target.results.first().copied(),
            old_mem: &mem_image,
            new_mem: &exec.new_mem,
            idx: None,
        };
        match eval_bool(&contract.post, &env) {
            Ok(true) => {}
            Ok(false) => return Verdict::Counterexample { args, trap: false },
            Err(_) => return Verdict::Unknown(UnknownReason::IllFormedExpr),
        }
        match frame_ok(&contract.frame, &env) {
            Ok(true) => {}
            Ok(false) => return Verdict::Counterexample { args, trap: false },
            Err(_) => return Verdict::Unknown(UnknownReason::IllFormedExpr),
        }
        passed += 1;
    }

    if passed == 0 {
        // Could not sample the precondition at all: either vacuous or the
        // sampler is too weak. Honest verdict: budget exceeded, not tested-0.
        return Verdict::Unknown(UnknownReason::BudgetExceeded);
    }
    Verdict::TestedN {
        n_cases: passed,
        seed: cfg.seed,
    }
}
