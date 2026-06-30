//! PDF shading rasterization for Type 2 (axial) and Type 3 (radial) shadings.
//!
//! References: PDF 1.7 spec §8.7.4.5 (Type 2) and §8.7.4.6 (Type 3).
//! Algorithm adapted from hayro `shading.rs` and pdf.js `pattern.js`.

/// Pre-parsed shading data ready for pixel sampling.
#[derive(Debug, Clone)]
pub struct Shading {
    pub kind: ShadingKind,
    /// Shading color space — only DeviceRGB / DeviceGray supported for now.
    pub n_components: usize,
    /// Optional clip rectangle in shading space [x0, y0, x1, y1].
    pub bbox: Option<[f64; 4]>,
    /// Pattern matrix (maps shading space → user space). Identity = [1,0,0,1,0,0].
    pub matrix: [f64; 6],
}

#[derive(Debug, Clone)]
pub enum ShadingKind {
    /// Type 2: linear gradient along a line from (x0,y0) to (x1,y1).
    Axial {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        t0: f64,
        t1: f64,
        function: ShadingFunction,
        extend_start: bool,
        extend_end: bool,
    },
    /// Type 3: radial gradient between two circles.
    Radial {
        x0: f64,
        y0: f64,
        r0: f64,
        x1: f64,
        y1: f64,
        r1: f64,
        t0: f64,
        t1: f64,
        function: ShadingFunction,
        extend_start: bool,
        extend_end: bool,
    },
}

/// A 1-in → N-out function used by shadings (Type 2 exponential or Type 4 PostScript).
#[derive(Debug, Clone)]
pub enum ShadingFunction {
    /// Exponential interpolation: y_j = C0_j + t^N * (C1_j - C0_j)
    Exponential { c0: Vec<f64>, c1: Vec<f64>, n: f64 },
    /// PostScript calculator function (Type 4).
    PostScript {
        program: Vec<u8>,
        domain: Vec<f64>,
        range: Vec<f64>,
    },
    /// Stitching function (Type 3) — combines multiple sub-functions.
    Stitching {
        functions: Vec<ShadingFunction>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
        domain: [f64; 2],
    },
    /// Sampled function (Type 0) — not yet implemented.
    Sampled { _marker: () },
}

impl ShadingFunction {
    /// Evaluate the function at parameter `t`, returning color components.
    pub fn eval(&self, t: f64) -> Vec<f64> {
        match self {
            ShadingFunction::Exponential { c0, c1, n } => {
                let t = t.clamp(0.0, 1.0);
                let s = if (n - 1.0).abs() < 1e-10 {
                    t
                } else {
                    t.powf(*n)
                };
                c0.iter()
                    .zip(c1.iter())
                    .map(|(a, b)| a + s * (b - a))
                    .collect()
            }
            ShadingFunction::Stitching {
                functions,
                bounds,
                encode,
                domain,
            } => {
                let t = t.clamp(domain[0], domain[1]);
                let mut idx = 0;
                for (i, &bound) in bounds.iter().enumerate() {
                    if t >= bound {
                        idx = i + 1;
                    }
                }
                if idx >= functions.len() {
                    idx = functions.len() - 1;
                }
                let d0 = if idx == 0 { domain[0] } else { bounds[idx - 1] };
                let d1 = if idx == bounds.len() {
                    domain[1]
                } else {
                    bounds[idx]
                };
                let t2 = if (d1 - d0).abs() < 1e-12 {
                    0.0
                } else {
                    (t - d0) / (d1 - d0)
                };
                let e0 = encode.get(idx * 2).copied().unwrap_or(0.0);
                let e1 = encode.get(idx * 2 + 1).copied().unwrap_or(1.0);
                let sub_t = e0 + t2 * (e1 - e0);
                functions[idx].eval(sub_t)
            }
            ShadingFunction::PostScript {
                program,
                domain: _,
                range: _,
            } => {
                let program = std::str::from_utf8(program).unwrap_or("");
                let mut stack = vec![t];
                for token in program
                    .replace(['{', '}'], " ")
                    .split_whitespace()
                    .filter(|t| !t.is_empty())
                {
                    if let Ok(v) = token.parse::<f64>() {
                        stack.push(v);
                        continue;
                    }
                    match token {
                        "dup" => {
                            if let Some(&v) = stack.last() {
                                stack.push(v);
                            }
                        }
                        "exch" => {
                            let len = stack.len();
                            if len >= 2 {
                                stack.swap(len - 1, len - 2);
                            }
                        }
                        "pop" => {
                            stack.pop();
                        }
                        "add" => bin_op(&mut stack, |a, b| a + b),
                        "sub" => bin_op(&mut stack, |a, b| a - b),
                        "mul" => bin_op(&mut stack, |a, b| a * b),
                        "div" => bin_op(&mut stack, |a, b| a / b),
                        "neg" => {
                            if let Some(v) = stack.last_mut() {
                                *v = -*v;
                            }
                        }
                        "abs" => {
                            if let Some(v) = stack.last_mut() {
                                *v = v.abs();
                            }
                        }
                        "sqrt" => {
                            if let Some(v) = stack.last_mut() {
                                *v = v.sqrt();
                            }
                        }
                        "exp" => bin_op(&mut stack, |a, b| a.powf(b)),
                        _ => {}
                    }
                }
                stack
            }
            ShadingFunction::Sampled { .. } => vec![0.0; 3],
        }
    }
}

fn bin_op(stack: &mut Vec<f64>, op: impl Fn(f64, f64) -> f64) {
    if stack.len() >= 2 {
        let b = stack.pop().unwrap();
        let a = stack.pop().unwrap();
        stack.push(op(a, b));
    }
}

impl Shading {
    /// Sample the shading at position (x, y) in shading space.
    /// Returns RGBA bytes (r, g, b, a) or None if outside the shading domain.
    pub fn sample(&self, x: f64, y: f64) -> Option<(u8, u8, u8, u8)> {
        // Check bbox
        if let Some([bx0, by0, bx1, by1]) = self.bbox {
            if x < bx0 || x > bx1 || y < by0 || y > by1 {
                return None;
            }
        }

        let t = match &self.kind {
            ShadingKind::Axial {
                x0,
                y0,
                x1,
                y1,
                t0,
                t1,
                extend_start,
                extend_end,
                ..
            } => {
                let dx = x1 - x0;
                let dy = y1 - y0;
                let len_sq = dx * dx + dy * dy;
                if len_sq < 1e-12 {
                    return None;
                }
                let t_raw = ((x - x0) * dx + (y - y0) * dy) / len_sq;
                // Map [0,1] → [t0,t1]
                let t_mapped = t0 + t_raw * (t1 - t0);

                // Clamp based on extend flags
                if t_raw < 0.0 && !extend_start {
                    return None;
                }
                if t_raw > 1.0 && !extend_end {
                    return None;
                }
                t_mapped.clamp((*t0).min(*t1), (*t0).max(*t1))
            }
            ShadingKind::Radial {
                x0,
                y0,
                r0,
                x1,
                y1,
                r1,
                t0,
                t1,
                extend_start,
                extend_end,
                ..
            } => {
                // Solve for parameter t where point (x,y) lies on the circle at t.
                // Circle(t) has center (x0 + t*(x1-x0), y0 + t*(y1-y0)) and
                // radius r0 + t*(r1-r0).
                //
                // (x - cx(t))^2 + (y - cy(t))^2 = r(t)^2
                //
                // This is a quadratic in t: a*t^2 + b*t + c = 0
                let dx = x1 - x0;
                let dy = y1 - y0;
                let dr = r1 - r0;

                let a = dx * dx + dy * dy - dr * dr;
                let b = 2.0 * (dx * (x0 - x) + dy * (y0 - y) + dr * r0);
                let c = (x0 - x) * (x0 - x) + (y0 - y) * (y0 - y) - r0 * r0;

                let t_raw = if a.abs() < 1e-12 {
                    // Linear case (concentric circles or equal radii)
                    if b.abs() < 1e-12 {
                        return None;
                    }
                    -c / b
                } else {
                    let disc = b * b - 4.0 * a * c;
                    if disc < 0.0 {
                        return None;
                    }
                    let sq = disc.sqrt();
                    let t1_quad = (-b + sq) / (2.0 * a);
                    let t2_quad = (-b - sq) / (2.0 * a);
                    // Choose the root closest to [0, 1]
                    let clamp = |t: f64| t.clamp(0.0, 1.0);
                    if (clamp(t1_quad) - t1_quad).abs() < (clamp(t2_quad) - t2_quad).abs() {
                        t1_quad
                    } else {
                        t2_quad
                    }
                };

                let t_mapped = t0 + t_raw * (t1 - t0);

                if t_raw < 0.0 && !extend_start {
                    return None;
                }
                if t_raw > 1.0 && !extend_end {
                    return None;
                }
                t_mapped.clamp((*t0).min(*t1), (*t0).max(*t1))
            }
        };

        // Normalize t to [0,1] for function evaluation
        let function = match &self.kind {
            ShadingKind::Axial { function, .. } | ShadingKind::Radial { function, .. } => function,
        };

        let components = function.eval(t);
        Some(components_to_rgba(&components, self.n_components))
    }
}

fn components_to_rgba(components: &[f64], n_components: usize) -> (u8, u8, u8, u8) {
    let clamp = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match n_components {
        1 => {
            let g = clamp(components.first().copied().unwrap_or(0.0));
            (g, g, g, 255)
        }
        3 => (
            clamp(components.first().copied().unwrap_or(0.0)),
            clamp(components.get(1).copied().unwrap_or(0.0)),
            clamp(components.get(2).copied().unwrap_or(0.0)),
            255,
        ),
        4 => {
            let c = components.first().copied().unwrap_or(0.0);
            let m = components.get(1).copied().unwrap_or(0.0);
            let y = components.get(2).copied().unwrap_or(0.0);
            let k = components.get(3).copied().unwrap_or(0.0);
            #[cfg(feature = "rendering")]
            {
                let (r, g, b) = crate::rendering::cmyk_icc::cmyk_pixel_to_rgb(
                    c as f32, m as f32, y as f32, k as f32,
                );
                (r, g, b, 255)
            }
            #[cfg(not(feature = "rendering"))]
            {
                let r = (1.0 - c) * (1.0 - k);
                let g = (1.0 - m) * (1.0 - k);
                let b = (1.0 - y) * (1.0 - k);
                (clamp(r), clamp(g), clamp(b), 255)
            }
        }
        _ => (0, 0, 0, 255),
    }
}
