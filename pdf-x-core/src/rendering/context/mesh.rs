//! Free-form / lattice mesh decoding and Coons/tensor patch interpolation (Type 4/5 shadings).

use super::*;

pub(super) fn copy_patch_control_points(
    flag: u32,
    prev: &[PatchPoint],
    current: &mut [PatchPoint],
) {
    match flag {
        1 => {
            current[0] = prev[3];
            current[1] = prev[4];
            current[2] = prev[5];
            current[3] = prev[6];
        }
        2 => {
            current[0] = prev[6];
            current[1] = prev[7];
            current[2] = prev[8];
            current[3] = prev[9];
        }
        3 => {
            current[0] = prev[9];
            current[1] = prev[10];
            current[2] = prev[11];
            current[3] = prev[0];
        }
        _ => {}
    }
}

pub(super) fn map_coons_patch(cp: &[PatchPoint], u: f64, v: f64) -> PatchPoint {
    let c1 = cubic_eval(cp[0], cp[11], cp[10], cp[9], u);
    let c2 = cubic_eval(cp[3], cp[4], cp[5], cp[6], u);
    let d1 = cubic_eval(cp[0], cp[1], cp[2], cp[3], v);
    let d2 = cubic_eval(cp[9], cp[8], cp[7], cp[6], v);
    let c10 = cubic_eval(cp[0], cp[11], cp[10], cp[9], 0.0);
    let c11 = cubic_eval(cp[0], cp[11], cp[10], cp[9], 1.0);
    let c20 = cubic_eval(cp[3], cp[4], cp[5], cp[6], 0.0);
    let c21 = cubic_eval(cp[3], cp[4], cp[5], cp[6], 1.0);

    let sc = PatchPoint {
        x: (1.0 - v) * c1.x + v * c2.x,
        y: (1.0 - v) * c1.y + v * c2.y,
    };
    let sd = PatchPoint {
        x: (1.0 - u) * d1.x + u * d2.x,
        y: (1.0 - u) * d1.y + u * d2.y,
    };
    let sb = PatchPoint {
        x: (1.0 - v) * ((1.0 - u) * c10.x + u * c11.x) + v * ((1.0 - u) * c20.x + u * c21.x),
        y: (1.0 - v) * ((1.0 - u) * c10.y + u * c11.y) + v * ((1.0 - u) * c20.y + u * c21.y),
    };

    PatchPoint {
        x: sc.x + sd.x - sb.x,
        y: sc.y + sd.y - sb.y,
    }
}

pub(super) fn map_tensor_patch(cp: &[PatchPoint], u: f64, v: f64) -> PatchPoint {
    pub(super) fn idx(i: usize, j: usize) -> usize {
        match (i, j) {
            (0, 0) => 0,
            (0, 1) => 1,
            (0, 2) => 2,
            (0, 3) => 3,
            (1, 0) => 11,
            (1, 1) => 12,
            (1, 2) => 13,
            (1, 3) => 4,
            (2, 0) => 10,
            (2, 1) => 15,
            (2, 2) => 14,
            (2, 3) => 5,
            (3, 0) => 9,
            (3, 1) => 8,
            (3, 2) => 7,
            (3, 3) => 6,
            _ => 0,
        }
    }

    let mut x = 0.0;
    let mut y = 0.0;
    for i in 0..4 {
        for j in 0..4 {
            let basis = bernstein(i, u) * bernstein(j, v);
            let p = cp[idx(i, j)];
            x += p.x * basis;
            y += p.y * basis;
        }
    }
    PatchPoint { x, y }
}

pub(super) fn interpolate_patch_color(colors: &[Vec<f64>; 4], u: f64, v: f64) -> Vec<f64> {
    let n = colors.iter().map(Vec::len).min().unwrap_or(0);
    (0..n)
        .map(|i| {
            (1.0 - u) * (1.0 - v) * colors[0][i]
                + u * (1.0 - v) * colors[3][i]
                + u * v * colors[2][i]
                + (1.0 - u) * v * colors[1][i]
        })
        .collect()
}

pub(super) fn cubic_eval(
    p0: PatchPoint,
    p1: PatchPoint,
    p2: PatchPoint,
    p3: PatchPoint,
    t: f64,
) -> PatchPoint {
    let mt = 1.0 - t;
    let b0 = mt.powi(3);
    let b1 = 3.0 * t * mt.powi(2);
    let b2 = 3.0 * t.powi(2) * mt;
    let b3 = t.powi(3);
    PatchPoint {
        x: b0 * p0.x + b1 * p1.x + b2 * p2.x + b3 * p3.x,
        y: b0 * p0.y + b1 * p1.y + b2 * p2.y + b3 * p3.y,
    }
}

pub(super) fn bernstein(i: usize, t: f64) -> f64 {
    match i {
        0 => (1.0 - t).powi(3),
        1 => 3.0 * t * (1.0 - t).powi(2),
        2 => 3.0 * t.powi(2) * (1.0 - t),
        3 => t.powi(3),
        _ => 0.0,
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PatchPoint {
    pub(super) x: f64,
    pub(super) y: f64,
}

pub(super) fn sample_max(bits: usize) -> Option<f64> {
    if bits == 0 || bits > 32 {
        None
    } else {
        Some((1u64.checked_shl(bits as u32)? - 1) as f64)
    }
}

pub(super) fn interpolate_number(x: f64, xmin: f64, xmax: f64, ymin: f64, ymax: f64) -> f64 {
    if (xmax - xmin).abs() < f64::EPSILON {
        ymin
    } else {
        ymin + (x - xmin) * (ymax - ymin) / (xmax - xmin)
    }
}

pub(super) struct MeshBitReader<'a> {
    pub(super) data: &'a [u8],
    pub(super) bit_pos: usize,
}

impl<'a> MeshBitReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub(super) fn remaining_bits(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.bit_pos)
    }

    pub(super) fn read(&mut self, bits: usize) -> Option<u32> {
        if bits > 32 || self.remaining_bits() < bits {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..bits {
            let byte = *self.data.get(self.bit_pos / 8)?;
            let bit = 7 - (self.bit_pos % 8);
            value = (value << 1) | (((byte >> bit) & 1) as u32);
            self.bit_pos += 1;
        }
        Some(value)
    }

    pub(super) fn align_byte(&mut self) {
        let rem = self.bit_pos % 8;
        if rem != 0 {
            self.bit_pos += 8 - rem;
        }
    }
}

/// Parse a shading function from a PDFObject (Type 2 exponential or Type 4 PostScript).
pub(super) fn parse_shading_function(
    obj: &PDFObject,
    resolver: &mut Option<&mut dyn PageResolver>,
) -> crate::rendering::shading::ShadingFunction {
    let resolved = resolve_object_clone(obj.clone(), resolver).unwrap_or(PDFObject::Null);
    match &resolved {
        PDFObject::Dictionary(dict) => {
            let ft = dict
                .get("FunctionType")
                .and_then(|v| v.as_number())
                .unwrap_or(2.0) as i32;
            if ft == 2 {
                let c0 = number_array(dict.get("C0")).unwrap_or_default();
                let c1 = number_array(dict.get("C1")).unwrap_or_default();
                let n = dict.get("N").and_then(|v| v.as_number()).unwrap_or(1.0);
                crate::rendering::shading::ShadingFunction::Exponential { c0, c1, n }
            } else if ft == 3 {
                let domain_arr = number_array(dict.get("Domain")).unwrap_or_else(|| vec![0.0, 1.0]);
                let domain = [
                    domain_arr.first().copied().unwrap_or(0.0),
                    domain_arr.get(1).copied().unwrap_or(1.0),
                ];
                let bounds = number_array(dict.get("Bounds")).unwrap_or_default();
                let encode = number_array(dict.get("Encode")).unwrap_or_default();
                let functions: Vec<crate::rendering::shading::ShadingFunction> =
                    match dict.get("Functions") {
                        Some(PDFObject::Array(arr)) => arr
                            .iter()
                            .map(|f| parse_shading_function(f, resolver))
                            .collect(),
                        _ => Vec::new(),
                    };
                crate::rendering::shading::ShadingFunction::Stitching {
                    functions,
                    bounds,
                    encode,
                    domain,
                }
            } else {
                crate::rendering::shading::ShadingFunction::Exponential {
                    c0: vec![0.0],
                    c1: vec![1.0],
                    n: 1.0,
                }
            }
        }
        PDFObject::Stream { dict, data } => {
            let ft = dict
                .get("FunctionType")
                .and_then(|v| v.as_number())
                .unwrap_or(4.0) as i32;
            if ft == 0 {
                parse_sampled_function(dict, data)
            } else {
                crate::rendering::shading::ShadingFunction::PostScript {
                    program: data.clone(),
                    domain: number_array(dict.get("Domain")).unwrap_or_default(),
                    range: number_array(dict.get("Range")).unwrap_or_default(),
                }
            }
        }
        _ => crate::rendering::shading::ShadingFunction::Exponential {
            c0: vec![0.0],
            c1: vec![1.0],
            n: 1.0,
        },
    }
}

pub(super) fn parse_sampled_function(
    dict: &HashMap<String, PDFObject>,
    data: &[u8],
) -> crate::rendering::shading::ShadingFunction {
    let domain = number_array(dict.get("Domain")).unwrap_or_default();
    let range = number_array(dict.get("Range")).unwrap_or_default();
    let size: Vec<usize> = number_array(dict.get("Size"))
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.max(1.0) as usize)
        .collect();
    let bits_per_sample = dict
        .get("BitsPerSample")
        .and_then(|v| v.as_number())
        .unwrap_or(8.0) as usize;

    let input_size = domain.len() / 2;
    let output_size = range.len() / 2;
    if input_size == 0 || output_size == 0 || size.len() < input_size {
        return crate::rendering::shading::ShadingFunction::Exponential {
            c0: vec![0.0],
            c1: vec![1.0],
            n: 1.0,
        };
    }

    let encode = number_array(dict.get("Encode")).unwrap_or_else(|| {
        size.iter()
            .take(input_size)
            .flat_map(|s| [0.0, s.saturating_sub(1) as f64])
            .collect()
    });
    let decode = number_array(dict.get("Decode")).unwrap_or_else(|| range.clone());
    let decoded = dict
        .get("Filter")
        .and_then(|filter| crate::core::decode::apply_filters(data, filter).ok())
        .unwrap_or_else(|| data.to_vec());
    let sample_count = size
        .iter()
        .take(input_size)
        .fold(output_size, |acc, s| acc.saturating_mul(*s));
    let samples = read_sample_array(&decoded, sample_count, bits_per_sample);

    crate::rendering::shading::ShadingFunction::Sampled {
        domain,
        range,
        size,
        bits_per_sample,
        encode,
        decode,
        samples,
    }
}

pub(super) fn read_sample_array(
    data: &[u8],
    sample_count: usize,
    bits_per_sample: usize,
) -> Vec<f64> {
    if sample_count == 0 || bits_per_sample == 0 || bits_per_sample > 32 {
        return Vec::new();
    }

    let sample_max = 2f64.powi(bits_per_sample as i32) - 1.0;
    let mut samples = Vec::with_capacity(sample_count);
    let mut code_buf = 0u64;
    let mut code_size = 0usize;
    let mut byte_index = 0usize;

    for _ in 0..sample_count {
        while code_size < bits_per_sample {
            code_buf = (code_buf << 8) | data.get(byte_index).copied().unwrap_or(0) as u64;
            byte_index += 1;
            code_size += 8;
        }

        code_size -= bits_per_sample;
        let value = (code_buf >> code_size) & ((1u64 << bits_per_sample) - 1);
        samples.push(value as f64 / sample_max);
        code_buf &= if code_size == 0 {
            0
        } else {
            (1u64 << code_size) - 1
        };
    }

    samples
}
