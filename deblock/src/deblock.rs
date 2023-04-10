//! Rust implementation of a deblocking filter inspired by ITU-T H.263 Annex J.
//! This is intended to be used as a postprocessing step, not as a loop filter.

/// Table J.2/H.263 - Relationship between QUANT and STRENGTH of filter; [0] is not to be used
pub const QUANT_TO_STRENGTH: [u8; 32] = [
    0, 1, 1, 2, 2, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10, 10, 11, 11, 11,
    12, 12, 12,
];

use std::ops::Shr;

use wide::i16x8;
use wide::u8x16;
use wide::CmpGt;
use wide::CmpLt;

/// Figure J.2/H.263 – Parameter d1 as a function of parameter d for deblocking filter mode
#[inline]
fn up_down_ramp(x: i16, strength: i16) -> i16 {
    x.signum() * (x.abs() - (2 * (x.abs() - strength)).max(0)).max(0)
}

fn signum_simd(x: i16x8) -> i16x8 {
    // NOTE: the "true" value of these comparisons is all 1 bits, which
    // is numerically -1, hence the reversed usage ot lt and gt.
    return (x.cmp_lt(i16x8::ZERO)) - (x.cmp_gt(i16x8::ZERO));
}

#[inline]
fn up_down_ramp_simd(x: i16x8, strength: i16) -> i16x8 {
    signum_simd(x)
        * (x.abs() - (2 * (x.abs() - i16x8::splat(strength))).max(i16x8::ZERO)).max(i16x8::ZERO)
}

/// Clips x to the range +/- abs(lim)
#[inline]
fn clipd1(x: i16, lim: i16) -> i16 {
    x.clamp(-lim.abs(), lim.abs())
}

#[inline]
fn clamp_simd(x: i16x8, min: i16x8, max: i16x8) -> i16x8 {
    x.max(min).min(max)
}

/// Clips x to the range +/- abs(lim)
#[inline]
fn clipd1_simd(x: i16x8, lim: i16x8) -> i16x8 {
    let la = lim.abs();
    clamp_simd(x, -la, la)
}

/// Operates the filter on a set of four (clipped) pixel values on a horizontal or
/// vertical line of the picture, of which A and B belong to one block, and C and D
/// belong to a neighbouring block which is to the right of or below the first block.
/// Figure J.1 shows examples for the position of these pixels.
#[allow(non_snake_case)]
#[inline]
fn process(A: &mut u8, B: &mut u8, C: &mut u8, D: &mut u8, strength: u8) {
    let a16 = *A as i16;
    let b16 = *B as i16;
    let c16 = *C as i16;
    let d16 = *D as i16;

    let d: i16 = (a16 - 4 * b16 + 4 * c16 - d16) / 8;
    let d1: i16 = up_down_ramp(d, strength as i16);
    let d2: i16 = clipd1((a16 - d16) / 4, d1 / 2);

    *A = (a16 - d2) as u8;
    *B = (b16 + d1).clamp(0, 255) as u8;
    *C = (c16 - d1).clamp(0, 255) as u8;
    *D = (d16 + d2) as u8;
}

#[allow(non_snake_case)]
#[inline]
fn process_simd(A: &mut [u8], B: &mut [u8], C: &mut [u8], D: &mut [u8], strength: u8) {
    let a16 = i16x8::from([
        A[0] as i16,
        A[1] as i16,
        A[2] as i16,
        A[3] as i16,
        A[4] as i16,
        A[5] as i16,
        A[6] as i16,
        A[7] as i16,
    ]);
    let b16 = i16x8::from([
        B[0] as i16,
        B[1] as i16,
        B[2] as i16,
        B[3] as i16,
        B[4] as i16,
        B[5] as i16,
        B[6] as i16,
        B[7] as i16,
    ]);
    let c16 = i16x8::from([
        C[0] as i16,
        C[1] as i16,
        C[2] as i16,
        C[3] as i16,
        C[4] as i16,
        C[5] as i16,
        C[6] as i16,
        C[7] as i16,
    ]);
    let d16 = i16x8::from([
        D[0] as i16,
        D[1] as i16,
        D[2] as i16,
        D[3] as i16,
        D[4] as i16,
        D[5] as i16,
        D[6] as i16,
        D[7] as i16,
    ]);

    let d: i16x8 = (a16 - 4 * b16 + 4 * c16 - d16).shr(3);
    let d1: i16x8 = up_down_ramp_simd(d, strength as i16);
    let d2: i16x8 = clipd1_simd((a16 - d16).shr(2), d1.shr(1));

    let res_a = a16 - d2;
    let res_b = clamp_simd(b16 + d1, i16x8::ZERO, i16x8::splat(255));
    let res_c = clamp_simd(c16 - d1, i16x8::ZERO, i16x8::splat(255));
    let res_d = d16 + d2;

    let res_a = res_a.as_array_ref();
    let res_b = res_b.as_array_ref();
    let res_c = res_c.as_array_ref();
    let res_d = res_d.as_array_ref();

    for i in 0..8 {
        A[i] = res_a[i] as u8;
        B[i] = res_b[i] as u8;
        C[i] = res_c[i] as u8;
        D[i] = res_d[i] as u8;
    }
}

#[inline(never)]
fn deblock_horiz(result: &mut [u8], width: usize, height: usize, strength: u8) {
    let mut edge_y = 8; // the index of the C sample
    while edge_y <= height - 2 {
        let (_, rest) = result.split_at_mut((edge_y - 2) * width);
        let (row_a, rest) = rest.split_at_mut(width);
        let (row_b, rest) = rest.split_at_mut(width);
        let (row_c, rest) = rest.split_at_mut(width);
        let (row_d, _) = rest.split_at_mut(width);

        let row_a = row_a.chunks_exact_mut(8);
        let row_b = row_b.chunks_exact_mut(8);
        let row_c = row_c.chunks_exact_mut(8);
        let row_d = row_d.chunks_exact_mut(8);

        for (((A, B), C), D) in row_a.zip(row_b).zip(row_c).zip(row_d) {
            process_simd(A, B, C, D, strength);
        }

        edge_y += 8;
    }
}

#[inline(never)]
fn deblock_vert(result: &mut [u8], width: usize, strength: u8) {
    // so the [6..] below doesn't panic, also not enough pixels to process any vertical edges otherwise
    if width >= 10 {
        for row in result.chunks_exact_mut(width) {
            for line in row[6..].chunks_exact_mut(4).step_by(2) {
                let mut a = line[0];
                let mut b = line[1];
                let mut c = line[2];
                let mut d = line[3];

                process(&mut a, &mut b, &mut c, &mut d, strength);

                line[0] = a;
                line[1] = b;
                line[2] = c;
                line[3] = d;

            }
        }
    }
}

/// Applies the deblocking filter to the horizontal and vertical block edges
/// of the given image data with the given strength, assuming 8x8 block size.
#[allow(non_snake_case)]
#[allow(clippy::identity_op)]
pub fn deblock(data: &[u8], width: usize, strength: u8) -> Vec<u8> {
    debug_assert!(data.len() % width == 0);
    let height = data.len() / width;

    let mut result = data.to_vec();

    //deblock_horiz(result.as_mut(), width, height, strength);

    deblock_vert(result.as_mut(), width, strength);

    result
}
