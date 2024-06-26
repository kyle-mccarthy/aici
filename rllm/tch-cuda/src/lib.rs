use std::fmt::Display;
use tch::{Kind, Tensor};
use torch_sys::C_tensor;
use rustc_hash::FxHashMap as HashMap;

mod event;
mod stream;

pub use event::*;
pub use stream::*;

unsafe fn ptr_to_string(ptr: *mut libc::c_char) -> Option<String> {
    if !ptr.is_null() {
        let str = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
        libc::free(ptr as *mut libc::c_void);
        Some(str)
    } else {
        None
    }
}

pub(crate) unsafe fn check_res(f: &str, res: *mut libc::c_char) {
    match ptr_to_string(res) {
        None => (),
        Some(err) => panic!("{}: {}", f, err),
    }
}

extern "C" {
    fn mha_varlen_fwd_C(
        q: *const C_tensor, // total_q x num_heads x head_size, total_q := \sum_{i=0}^{b} s_i
        k: *const C_tensor, // total_k x num_heads_k x head_size, total_k := \sum_{i=0}^{b} s_i
        v: *const C_tensor, // total_k x num_heads_k x head_size, total_k := \sum_{i=0}^{b} s_i
        cu_seqlens_q: *const C_tensor, // b+1
        cu_seqlens_k: *const C_tensor, // b+1
        seqused_k: *const C_tensor, // b. If given, only this many elements of each batch element's keys are used. (opt)
        max_seqlen_q: i32,
        max_seqlen_k: i32,
        p_dropout: f32,
        softmax_scale: f32,
        zero_tensors: bool,
        is_causal: bool,
        window_size_left: i32,
        window_size_right: i32,
        outp: *mut *mut C_tensor, // total_q x num_heads x head_size, total_k := \sum_{i=0}^{b} s_i
    ) -> *mut libc::c_char;
}

/// Flash-attention v2 layer with variable-length batching.
///
/// This implements scaled dot-product attention, `softmax(Q @ K^T . softmax_scale) @ V`.
/// Multi-query and grouped-query attention are supported by using tensors k and v with fewer heads
/// than q, the number of heads in k and v has to be divisible by the number of heads in q.
///
/// # Arguments
///
/// * `q` - Query tensor with shape `(total_q, num_heads_q, head_size)`.
/// * `k` - Key tensor with shape `(total_kv, num_heads_kv, head_size)`.
/// * `v` - Value tensor with shape `(total_kv, num_heads_kv, head_size)`.
/// * `seqlens_q` - The cumulative lengths of the sequences in the batch, used to index in q.
/// * `seqlens_k` - The cumulative lengths of the sequences in the batch, used to index in k and v.
/// * `max_seqlen_q` - The maximum query sequence length for q in the batch.
/// * `max_seqlen_k` - The maximum query sequence length for k and v in the batch.
///
/// `seqlens_q` and `seqlens_k` contain `batch_size + 1` elements, typically `0`, `seqlen_1`,
/// `seqlen_1 + seqlen_2`, etc.
///
/// The resulting tensor has dimensions `(total_q, num_heads_q, head_size)`.
pub fn flash_attn_varlen(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    seqlens_q: &Tensor,
    seqlens_k: &Tensor,
    max_seqlen_q: usize,
    max_seqlen_k: usize,
    softmax_scale: f32,
    causal: bool,
) -> Tensor {
    let mut outputs = vec![std::ptr::null_mut(); 1];
    let err = unsafe {
        ptr_to_string(mha_varlen_fwd_C(
            q.as_ptr(),
            k.as_ptr(),
            v.as_ptr(),
            seqlens_q.as_ptr(),
            seqlens_k.as_ptr(),
            std::ptr::null(),
            max_seqlen_q as i32,
            max_seqlen_k as i32,
            0.0,
            softmax_scale,
            false,
            causal,
            -1,
            -1,
            outputs.as_mut_ptr(),
        ))
    };
    match err {
        None => unsafe { Tensor::from_ptr(outputs[0]) },
        Some(err) => panic!("flash_attn_varlen: {}", err),
    }
}

#[allow(dead_code)]
extern "C" {
    fn paged_attention_v1_C(
        out: *mut C_tensor,
        query: *const C_tensor,
        key_cache: *const C_tensor,
        value_cache: *const C_tensor,
        num_kv_heads: i32,
        scale: f32,
        block_tables: *const C_tensor,
        context_lens: *const C_tensor,
        block_size: i32,
        max_context_len: i32,
        alibi_slopes: *const C_tensor,
    ) -> *mut libc::c_char;

    fn paged_attention_v2_C(
        out: *mut C_tensor,
        exp_sums: *mut C_tensor,
        max_logits: *mut C_tensor,
        tmp_out: *mut C_tensor,
        query: *mut C_tensor,
        key_cache: *mut C_tensor,
        value_cache: *mut C_tensor,
        num_kv_heads: i32,
        scale: f32,
        block_tables: *mut C_tensor,
        context_lens: *mut C_tensor,
        block_size: i32,
        max_context_len: i32,
        alibi_slopes: *const C_tensor,
    ) -> *mut libc::c_char;

    fn rms_norm_C(
        out: *mut C_tensor,
        input: *mut C_tensor,
        weight: *mut C_tensor,
        epsilon: f32,
    ) -> *mut libc::c_char;

    fn fused_add_rms_norm_C(
        input: *mut C_tensor,
        residual: *mut C_tensor,
        weight: *mut C_tensor,
        epsilon: f32,
    ) -> *mut libc::c_char;

    fn rotary_embedding_C(
        positions: *const C_tensor,
        query: *mut C_tensor,
        key: *mut C_tensor,
        head_size: i32,
        cos_sin_cache: *const C_tensor,
        is_neox: bool,
    ) -> *mut libc::c_char;

    fn silu_and_mul_C(out: *mut C_tensor, input: *mut C_tensor) -> *mut libc::c_char;

    fn gelu_new_C(out: *mut C_tensor, input: *mut C_tensor) -> *mut libc::c_char;

    fn gelu_fast_C(out: *mut C_tensor, input: *mut C_tensor) -> *mut libc::c_char;

    fn reshape_and_cache_C(
        key: *const C_tensor,
        value: *const C_tensor,
        key_cache: *mut C_tensor,
        value_cache: *mut C_tensor,
        slot_mapping: *const C_tensor,
    ) -> *mut libc::c_char;

    fn gather_cached_kv_C(
        key: *mut C_tensor,
        value: *mut C_tensor,
        key_cache: *const C_tensor,
        value_cache: *const C_tensor,
        slot_mapping: *const C_tensor,
    ) -> *mut libc::c_char;

    fn copy_blocks_2_C(
        key_cache_ptrs_tensor: *const C_tensor,
        value_cache_ptrs_tensor: *const C_tensor,
        block_mapping_tensor: *const C_tensor,
        key0: *const C_tensor,
    ) -> *mut libc::c_char;
}

pub fn reshape_and_cache(
    key: &Tensor,             // [num_tokens, num_heads, head_size]
    value: &Tensor,           // [num_tokens, num_heads, head_size]
    key_cache: &mut Tensor,   // [num_blocks, num_heads, head_size/x, block_size, x]
    value_cache: &mut Tensor, // [num_blocks, num_heads, head_size, block_size]
    slot_mapping: &Tensor,    // [num_tokens], int
) {
    // it's int64 in here, but int32 in gather*; go figure
    let slot_mapping = slot_mapping.to_kind(Kind::Int64);
    unsafe {
        check_res(
            "reshape_and_cache_C",
            reshape_and_cache_C(
                key.as_ptr(),
                value.as_ptr(),
                key_cache.as_mut_ptr(),
                value_cache.as_mut_ptr(),
                slot_mapping.as_ptr(),
            ),
        );
    }
}

pub fn gather_cached_kv(
    key: &mut Tensor,      // [num_tokens, num_heads, head_size]
    value: &mut Tensor,    // [num_tokens, num_heads, head_size]
    key_cache: &Tensor,    // [num_blocks, num_heads, head_size/x, block_size, x]
    value_cache: &Tensor,  // [num_blocks, num_heads, head_size, block_size]
    slot_mapping: &Tensor, // [num_tokens], int
) {
    unsafe {
        check_res(
            "gather_cached_kv_C",
            gather_cached_kv_C(
                key.as_mut_ptr(),
                value.as_mut_ptr(),
                key_cache.as_ptr(),
                value_cache.as_ptr(),
                slot_mapping.as_ptr(),
            ),
        );
    }
}

pub fn swap_blocks(
    _src: &Tensor,
    _dst: &Tensor,
    _block_mapping: &HashMap<usize, usize>,
    _stream: &CudaStream,
) {
    todo!()
}

fn to_cuda_ptr(t: &Tensor) -> i64 {
    t.data_ptr() as i64
}

fn is_bf16_or_f16(t: &Tensor) -> bool {
    match t.kind() {
        Kind::BFloat16 => true,
        Kind::Half => true,
        _ => false,
    }
}

fn check_cont_bf16_or_f16(t: &Tensor) {
    assert!(is_bf16_or_f16(t));
    assert!(t.device().is_cuda());
    assert!(t.is_contiguous());
}

// fn is_u32(t: &Tensor) -> bool {
//     match t.kind() {
//         Kind::Int => true,
//         _ => false,
//     }
// }

// fn check_cont_u32(t: &Tensor) {
//     assert!(is_u32(t));
//     assert!(t.device().is_cuda());
//     assert!(t.is_contiguous());
// }

pub fn copy_blocks(
    key_caches: &mut Vec<Tensor>,
    value_caches: &mut Vec<Tensor>,
    block_mapping: &HashMap<usize, Vec<usize>>,
) {
    let num_layers = key_caches.len();
    assert_eq!(num_layers, value_caches.len());
    if num_layers == 0 {
        return;
    }
    let device = key_caches[0].device();
    assert!(device.is_cuda());

    let (_num_blocks, num_heads, head_size, block_size) = value_caches[0].size4().unwrap();
    let _numel_per_block = (num_heads * head_size * block_size) as i32;

    let tsize = key_caches[0].numel();

    let key_cache_ptrs: Vec<i64> = key_caches.iter().map(|t| to_cuda_ptr(t)).collect();
    let value_cache_ptrs: Vec<i64> = value_caches.iter().map(|t| to_cuda_ptr(t)).collect();

    for layer_idx in 0..(2 * num_layers) {
        let e = if layer_idx < num_layers {
            &key_caches[layer_idx]
        } else {
            &value_caches[layer_idx - num_layers]
        };
        assert!(e.device() == device);
        assert_eq!(e.numel(), tsize);
        check_cont_bf16_or_f16(e);
    }

    let mut block_mapping_vec = Vec::new();
    for (&src_block_number, dst_block_numbers) in block_mapping {
        for &dst_block_number in dst_block_numbers {
            block_mapping_vec.push(src_block_number as i64);
            block_mapping_vec.push(dst_block_number as i64);
        }
    }

    let key_cache_ptrs_tensor = Tensor::from_slice(&key_cache_ptrs).to(device);
    let value_cache_ptrs_tensor = Tensor::from_slice(&value_cache_ptrs).to(device);
    let block_mapping_tensor = Tensor::from_slice(&block_mapping_vec).to(device);

    unsafe {
        check_res(
            "copy_blocks_2_C",
            copy_blocks_2_C(
                key_cache_ptrs_tensor.as_ptr(),
                value_cache_ptrs_tensor.as_ptr(),
                block_mapping_tensor.as_ptr(),
                key_caches[0].as_ptr(),
            ),
        );
    }
}

pub fn rotary_embedding(
    positions: &Tensor, // [batch_size, seq_len] or [num_tokens]
    query: &mut Tensor, // [batch_size, seq_len, num_heads * head_size] or [num_tokens, num_heads * head_size]
    key: &mut Tensor, // [batch_size, seq_len, num_kv_heads * head_size] or [num_tokens, num_kv_heads * head_size]
    head_size: usize,
    cos_sin_cache: &Tensor, // [max_position, rot_dim]
    is_neox: bool,
) {
    unsafe {
        check_res(
            "rotary_embedding_C",
            rotary_embedding_C(
                positions.as_ptr(),
                query.as_mut_ptr(),
                key.as_mut_ptr(),
                head_size as i32,
                cos_sin_cache.as_ptr(),
                is_neox,
            ),
        );
    }
}

pub fn paged_attention_v1(
    out: &mut Tensor,     // [num_seqs, num_heads, head_size]
    query: &Tensor,       // [num_seqs, num_heads, head_size]
    key_cache: &Tensor,   // [num_blocks, num_heads, head_size/x, block_size, x]
    value_cache: &Tensor, // [num_blocks, num_heads, head_size, block_size]
    num_kv_heads: usize,
    scale: f32,
    block_tables: &Tensor, // [num_seqs, max_num_blocks_per_seq], int
    context_lens: &Tensor, // [num_seqs], int
    block_size: usize,
    max_context_len: usize,
    alibi_slopes: Option<&Tensor>,
) {
    let alibi_slopes = match alibi_slopes {
        None => std::ptr::null(),
        Some(t) => t.as_ptr(),
    };
    unsafe {
        check_res(
            "paged_attention_v1_C",
            paged_attention_v1_C(
                out.as_mut_ptr(),
                query.as_ptr(),
                key_cache.as_ptr(),
                value_cache.as_ptr(),
                num_kv_heads as i32,
                scale,
                block_tables.as_ptr(),
                context_lens.as_ptr(),
                block_size as i32,
                max_context_len as i32,
                alibi_slopes,
            ),
        );
    }
}

#[repr(C)]
#[derive(Clone, Default)]
pub struct Stats {
    pub current: i64,
    pub peak: i64,
    pub allocated: i64,
    pub freed: i64,
}

#[repr(C)]
#[derive(Clone, Default)]
pub struct CudaProps {
    pub major: i32,
    pub minor: i32,
    pub multi_processor_count: i32,
    pub max_threads_per_multi_processor: i32,
    pub total_memory: i64,
}

impl Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const G: f64 = 1024.0 * 1024.0 * 1024.0;
        write!(
            f,
            "current: {:.3}GiB, peak: {:.3}GiB, allocated: {:.3}GiB, freed: {:.3}GiB",
            self.current as f64 / G,
            self.peak as f64 / G,
            self.allocated as f64 / G,
            self.freed as f64 / G
        )
    }
}

extern "C" {
    fn cuda_reset_peak_memory_stats_C(device: i32) -> *mut libc::c_char;
    fn cuda_empty_cache_C() -> *mut libc::c_char;
    fn cuda_get_stats_allocated_bytes_C(device: i32, outp: *mut Stats) -> *mut libc::c_char;
    fn cuda_get_device_properties_C(device: i32, outp: *mut CudaProps) -> *mut libc::c_char;
}

pub fn cuda_reset_peak_memory_stats(device: usize) {
    unsafe {
        check_res(
            "cuda_reset_peak_memory_stats",
            cuda_reset_peak_memory_stats_C(device as i32),
        );
    }
}

pub fn cuda_empty_cache() {
    unsafe {
        check_res("cuda_empty_cache", cuda_empty_cache_C());
    }
}

pub fn cuda_get_stats_allocated_bytes(device: usize) -> Stats {
    let mut stats = Stats::default();
    unsafe {
        check_res(
            "cuda_get_stats_allocated_bytes",
            cuda_get_stats_allocated_bytes_C(device as i32, &mut stats),
        );
    }
    stats
}

pub fn cuda_get_device_properties(device: usize) -> CudaProps {
    let mut props = CudaProps::default();
    unsafe {
        check_res(
            "cuda_get_device_properties",
            cuda_get_device_properties_C(device as i32, &mut props),
        );
    }
    props
}
