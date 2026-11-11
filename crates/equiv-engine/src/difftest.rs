//! Deterministic differential testing: seeded input generation, execution in
//! an embedded interpreter (wasmi; pure Rust, keeps the single-static-binary
//! constraint), contract evaluation per case, frame checking. Produces
//! `tested-N` / `counterexample` verdicts, never `proved` (AC-3).

use crate::eval::{eval_bool, Env};
use equiv_core::ast::*;
use equiv_core::verdict::Verdict;
use equiv_core::UnknownReason;

/// xorshift64*: deterministic, dependency-free. The seed is recorded in the
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
/// offsets, and full-width randoms. Pointer-shaped args must land inside
/// linear memory often enough for rejection sampling to work. The class and
/// the value come from separate draws: taken from one draw, the class fixes
/// the value's low bits (aligned offsets were never produced). With no
/// memory there is no offset class; it would only ever yield 0.
fn sample_arg(rng: &mut Rng, ty: ValType, mem_len: usize) -> u64 {
    let class = rng.next() >> 62; // 0..4
    let r = rng.next();
    let v = match class {
        0 => r % 256,
        1 | 2 if mem_len > 0 => r % mem_len as u64,
        1 => r % 256,
        _ => r,
    };
    match ty {
        ValType::I32 => v & 0xffff_ffff,
        _ => v,
    }
}

/// The values every run tries before any random one: 0, 1, -1, MIN, MAX at
/// the parameter's width.
fn boundary_values(ty: ValType) -> [u64; 5] {
    match ty {
        ValType::I32 => [0, 1, 0xffff_ffff, 0x8000_0000, 0x7fff_ffff],
        _ => [0, 1, u64::MAX, 1 << 63, (1 << 63) - 1],
    }
}

/// Inputs tried first, in a fixed order. Every boundary value appears in
/// every parameter position. When all combinations fit in half the case
/// budget they are all tried; otherwise the five same-boundary tuples, then
/// each boundary in each position with the other positions sampled, up to
/// half the budget.
fn forced_inputs(params: &[ValType], rng: &mut Rng, mem_len: usize, n_cases: u64) -> Vec<Vec<u64>> {
    let k = params.len();
    if k == 0 {
        return Vec::new();
    }
    let half = n_cases / 2;
    let product = 5u64.checked_pow(k as u32).filter(|p| *p <= half);
    let mut out: Vec<Vec<u64>> = Vec::new();
    if let Some(total) = product {
        for mut idx in 0..total {
            let mut tuple = vec![0u64; k];
            for (slot, ty) in tuple.iter_mut().zip(params).rev() {
                *slot = boundary_values(*ty)[(idx % 5) as usize];
                idx /= 5;
            }
            out.push(tuple);
        }
        return out;
    }
    for b in 0..5 {
        out.push(params.iter().map(|t| boundary_values(*t)[b]).collect());
    }
    'fill: for p in 0..k {
        for b in 0..5 {
            if out.len() as u64 >= half {
                break 'fill;
            }
            out.push(
                params
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        if i == p {
                            boundary_values(*t)[b]
                        } else {
                            sample_arg(rng, *t, mem_len)
                        }
                    })
                    .collect(),
            );
        }
    }
    out
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

    // The module's linear memory, under whatever name it is exported.
    let memory = instance.exports(&store).find_map(|e| e.into_memory());
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
        // Only a WASM trap is a trap. Any other failure (a signature the
        // call could not be made with, say) means the case did not run.
        Err(wasmi::Error::Trap(_)) => Ok(Execution {
            result: None,
            new_mem,
            trapped: true,
        }),
        Err(e) => Err(format!("call could not be made: {e}")),
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

/// What a concrete replay of solver-model arguments showed.
pub struct Replay {
    /// The model genuinely violates the contract: the pre holds, and the
    /// post fails or the call traps.
    pub violates: bool,
    /// The artifact trapped on these arguments.
    pub trapped: bool,
}

/// Concretely replay solver-model arguments against the artifact (AC-4).
/// Scalar contracts only; memory left at module defaults.
pub fn replay_scalar(wasm: &[u8], contract: &Contract, args: &[u64]) -> Option<Replay> {
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
        // model does not even satisfy the pre
        return Some(Replay { violates: false, trapped: false });
    }
    let exec = run_case(wasm, target, args, &mem).ok()?;
    if exec.trapped {
        return Some(Replay { violates: true, trapped: true });
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
    Some(Replay {
        violates: !eval_bool(&contract.post, &env).ok()?,
        trapped: false,
    })
}

/// Run the differential path. The wasm module must already have passed static
/// flag checks; float routing happens in the caller.
pub fn difftest(wasm: &[u8], contract: &Contract, cfg: &DiffConfig) -> Verdict {
    difftest_explained(wasm, contract, cfg).0
}

/// `difftest`, plus a sentence saying why when the verdict is `Unknown`, or
/// how many drawn inputs the precondition excluded when it is tested-N.
/// The sentence is not part of the receipt.
pub fn difftest_explained(wasm: &[u8], contract: &Contract, cfg: &DiffConfig) -> (Verdict, Option<String>) {
    let unknown = |r: UnknownReason, why: String| (Verdict::Unknown(r), Some(why));
    let target = &contract.target;

    // A memory that cannot be read from outside cannot be compared before
    // and after, so the frame clause (every contract has one) would pass
    // unexamined. Only a function with no memory, call or global
    // instruction at all is known not to need the comparison.
    match crate::wasm::analyze(wasm) {
        Ok(facts) => {
            // Direct callers get the same signature gate as `check`.
            if let Some((reason, why)) = crate::wasm::signature_problem(&facts, target) {
                return unknown(reason, why);
            }
            if facts.memories > 0
                && !facts.memory_exported
                && !crate::bmc::touches_no_memory(wasm, &target.name)
            {
                return unknown(
                    UnknownReason::UnsupportedFeature,
                    "the module has a linear memory but does not export it, so its bytes cannot be compared                      before and after the call: the frame clause was not evaluated"
                        .into(),
                );
            }
        }
        Err(_) => {
            return unknown(UnknownReason::IllFormedExpr, "the artifact is not a readable WASM module".into())
        }
    }

    // A zero state would make xorshift return 0 forever.
    let mut rng = Rng(if cfg.seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { cfg.seed });

    // Determine memory size from a probe instantiation.
    let probe = match run_case(wasm, target, &vec![0; target.params.len()], &[]) {
        Ok(e) => e,
        Err(e) => return unknown(UnknownReason::IllFormedExpr, format!("the artifact could not be run: {e}")),
    };
    let mem_len = probe.new_mem.len();

    let mut forced = forced_inputs(&target.params, &mut rng, mem_len, cfg.n_cases).into_iter();
    let mut passed = 0u64;
    let mut attempts = 0u64;
    let mut pre_false = 0u64;
    let mut pre_error = 0u64;
    while passed < cfg.n_cases && attempts < cfg.max_attempts {
        attempts += 1;
        let args: Vec<u64> = match forced.next() {
            Some(a) => a,
            None => target
                .params
                .iter()
                .map(|t| sample_arg(&mut rng, *t, mem_len))
                .collect(),
        };
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
            Ok(false) => {
                pre_false += 1; // pre unsatisfied: skip case
                continue;
            }
            Err(_) => {
                pre_error += 1; // pre not evaluable here: skip
                continue;
            }
        }

        let exec = match run_case(wasm, target, &args, &mem_image) {
            Ok(e) => e,
            Err(e) => return unknown(UnknownReason::IllFormedExpr, format!("the artifact could not be run: {e}")),
        };

        // Trap under a satisfied pre is a violation (spec §3: no-trap is the
        // default obligation).
        if exec.trapped {
            return (Verdict::Counterexample { args, trap: true }, None);
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
            Ok(false) => return (Verdict::Counterexample { args, trap: false }, None),
            Err(e) => {
                return unknown(
                    UnknownReason::IllFormedExpr,
                    format!("the postcondition could not be evaluated on input {args:?}: {e:?}"),
                )
            }
        }
        match frame_ok(&contract.frame, &env) {
            Ok(true) => {}
            Ok(false) => return (Verdict::Counterexample { args, trap: false }, None),
            Err(e) => {
                return unknown(
                    UnknownReason::IllFormedExpr,
                    format!("the frame clause could not be evaluated on input {args:?}: {e:?}"),
                )
            }
        }
        passed += 1;
    }

    if passed == 0 || passed < cfg.n_cases {
        // Fewer inputs than asked for reached the artifact: either the pre
        // is (nearly) vacuous or the sampler is too weak for it. Reporting
        // the handful that did run as tested-N would overstate the run.
        return unknown(
            UnknownReason::BudgetExceeded,
            format!(
                "only {passed} of the {} requested inputs satisfied the precondition in {attempts} attempts                  ({pre_false} did not satisfy it, {pre_error} could not be evaluated); none of the {passed}                  that ran violated the contract",
                cfg.n_cases
            ),
        );
    }
    // A pass that skipped inputs says so: the count of cases is exact, but
    // the reader should know how much of what was drawn the pre excluded.
    let note = (pre_false + pre_error > 0).then(|| {
        format!(
            "{passed} inputs ran; {pre_false} more were drawn but did not satisfy the precondition and \
             {pre_error} more could not have it evaluated"
        )
    });
    (
        Verdict::TestedN {
            n_cases: passed,
            seed: cfg.seed,
        },
        note,
    )
}
