//! Rust implementation of a deblocking filter inspired by ITU-T H.263 Annex J.
//! This is intended to be used as a postprocessing step, not as a loop filter.

/// Table J.2/H.263 - Relationship between QUANT and STRENGTH of filter; [0] is not to be used
pub const QUANT_TO_STRENGTH: [u8; 32] = [
    0, 1, 1, 2, 2, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10, 10, 11, 11, 11,
    12, 12, 12,
];

/// Figure J.2/H.263 – Parameter d1 as a function of parameter d for deblocking filter mode
#[inline]
fn up_down_ramp(x: i32, strength: i32) -> i32 {
    x.signum() * (x.abs() - (2 * (x.abs() - strength)).max(0)).max(0)
}

/// Clips x to the range +/- abs(lim)
#[inline]
fn clipd1(x: i32, lim: i32) -> i32 {
    x.clamp(-lim.abs(), lim.abs())
}

/// Operates the filter on a set of four (clipped) pixel values on a horizontal or
/// vertical line of the picture, of which A and B belong to one block, and C and D
/// belong to a neighbouring block which is to the right of or below the first block.
/// Figure J.1 shows examples for the position of these pixels.
#[allow(non_snake_case)]
#[inline]
fn process(A: &mut u8, B: &mut u8, C: &mut u8, D: &mut u8, strength: u8) {
    let d: i32 = (*A as i32 - 4 * *B as i32 + 4 * *C as i32 - *D as i32) / 8;
    let d1: i32 = up_down_ramp(d, strength as i32);
    let d2: i32 = clipd1((*A as i32 - *D as i32) / 4, d1 / 2);

    let B1: i32 = (*B as i32 + d1).clamp(0, 255);
    let C1: i32 = (*C as i32 - d1).clamp(0, 255);
    let A1: i32 = *A as i32 - d2;
    let D1: i32 = *D as i32 + d2;

    *A = A1 as u8;
    *B = B1 as u8;
    *C = C1 as u8;
    *D = D1 as u8;
}

/// Applies the deblocking filter to the horizontal and vertical block edges
/// of the given image data with the given strength, assuming 8x8 block size.
#[allow(non_snake_case)]
#[allow(clippy::identity_op)]
pub fn deblock(data: &[u8], width: usize, strength: u8) -> Vec<u8> {
    debug_assert!(data.len() % width == 0);
    let height = data.len() / width;

    let mut result = data.to_vec();

    // horizontal edges
    let mut edge_y = 8; // the index of the C sample
    while edge_y <= height - 2 {

        let (_, rest) = result.split_at_mut((edge_y - 2) * width);
        let (row_A, rest) = rest.split_at_mut(width);
        let (row_B, rest) = rest.split_at_mut(width);
        let (row_C, rest) = rest.split_at_mut(width);
        let (row_D, _) = rest.split_at_mut(width);

        for (((A, B), C), D) in row_A.iter_mut().zip(row_B).zip(row_C).zip(row_D) {
            process(A, B, C, D, strength);
        }

        edge_y += 8;
    }

    // vertical edges
    let mut edge_x = 8; // the index of the C sample
    while edge_x <= width - 2 {
        for y in 0..height {
            let idx_A = y * width + edge_x - 2;
            let idx_B = y * width + edge_x - 1;
            let idx_C = y * width + edge_x + 0;
            let idx_D = y * width + edge_x + 1;

            let mut A = result[idx_A];
            let mut B = result[idx_B];
            let mut C = result[idx_C];
            let mut D = result[idx_D];

            process(&mut A, &mut B, &mut C, &mut D, strength);

            result[idx_A] = A;
            result[idx_B] = B;
            result[idx_C] = C;
            result[idx_D] = D;
        }

        edge_x += 8;
    }

    result
}
