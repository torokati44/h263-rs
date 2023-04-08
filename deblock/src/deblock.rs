//! Rust implementation of a deblocking filter inspired by ITU-T H.263 Annex J.
//! This is intended to be used as a postprocessing step, not as a loop filter.

/// Table J.2/H.263 - Relationship between QUANT and STRENGTH of filter; [0] is not to be used
pub const QUANT_TO_STRENGTH: [u8; 32] = [
    0, 1, 1, 2, 2, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10, 10, 11, 11, 11,
    12, 12, 12,
];

/// Figure J.2/H.263 – Parameter d1 as a function of parameter d for deblocking filter mode
#[inline]
fn up_down_ramp(x: i16, strength: i16) -> i16 {
    x.signum() * (x.abs() - (2 * (x.abs() - strength)).max(0)).max(0)
}

/// Clips x to the range +/- abs(lim)
#[inline]
fn clipd1(x: i16, lim: i16) -> i16 {
    x.clamp(-lim.abs(), lim.abs())
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

    // so the [6..] below doesn't panic, also not enough pixels to process any vertical edges otherwise
    if width >= 10 {
        // vertical edges
        for row in result.chunks_exact_mut(width) {
            for line in row[6..].chunks_exact_mut(4).step_by(2) {
                let (a, line) = line.split_first_mut().unwrap();
                let (b, line) = line.split_first_mut().unwrap();
                let (c, line) = line.split_first_mut().unwrap();
                let (d, _) = line.split_first_mut().unwrap();
                process(a, b, c, d, strength)
            }
        }
    }

    result
}
