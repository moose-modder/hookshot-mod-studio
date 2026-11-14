//! A B-spline fitted to samples within an error bound: the curve the spline encoder stores for one part of one track in
//! one block ([`approximate`]).
//!
//! The fit is the error-bounded global approximation of *The NURBS Book* (Piegl and Tiller, 2nd ed., section 9.4.4,
//! algorithm A9.10, with A9.8/A9.9 knot removal, A5.8 knot removal, A5.9 degree elevation and A2.1-A2.4 basis
//! functions): start from the polyline through every sample, and at each degree remove every knot whose removal keeps
//! each sample's accumulated error within the bound, then raise the degree by one (least-squares fit on the knots with
//! every multiplicity raised, or exact degree elevation when that fit fails), and finally refit and remove once more at
//! the target degree. Samples keep their own parameters (the frame times); the error of a sample is the Euclidean
//! distance in four dimensions (a quaternion's `w` counts; a position's or scale's is zero). Everything is computed in
//! f32, one operation at a time, in a fixed order, so the same samples give the same curve on every machine.
//!
//! The steps and constants are the ones the compressor's output shows (tests/compress_anim.rs gives the same bytes): the
//! least-squares system is solved by conjugate gradients on the normal equations with at most 50 iterations, the
//! residual recomputed every 50, a squared-residual bound of `1e-10`; every non-empty knot span must hold a sample
//! further than `1e-5` from its ends; a fit is accepted when every sample is within 1.1 times the bound.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::{Error, Result, invalid};

/// A point of the fit: x, y, z, w.
pub(crate) type Point = [f32; 4];

/// The highest degree the fit makes (the stored format's knots support 1 to 3).
pub(crate) const MAX_DEGREE: usize = 3;

/// A failure of an index or count the fit keeps itself: a bug, reported instead of a panic.
fn bug(what: &str) -> Error {
    invalid(0, format!("spline fit: {what}"))
}

fn at<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i).copied().ok_or_else(|| bug("index outside a vector"))
}

fn slot<T>(v: &mut [T], i: usize) -> Result<&mut T> {
    v.get_mut(i).ok_or_else(|| bug("index outside a vector"))
}

fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b).ok_or_else(|| bug("index overflow"))
}

fn sub(a: usize, b: usize) -> Result<usize> {
    a.checked_sub(b).ok_or_else(|| bug("index underflow"))
}

fn iadd(a: isize, b: isize) -> Result<isize> {
    a.checked_add(b).ok_or_else(|| bug("index overflow"))
}

fn isub(a: isize, b: isize) -> Result<isize> {
    a.checked_sub(b).ok_or_else(|| bug("index overflow"))
}

fn scaled(p: Point, s: f32) -> Point {
    p.map(|c| c * s)
}

fn minus(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

fn plus(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn divided(p: Point, s: f32) -> Point {
    p.map(|c| c / s)
}

/// `a . b`, summed from x to w.
pub(crate) fn dot(a: Point, b: Point) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// The length of `a - b`.
pub(crate) fn distance(a: Point, b: Point) -> f32 {
    let d = minus(a, b);
    dot(d, d).sqrt()
}

/// A B-spline: its degree, its knots (`points + degree + 1`, not decreasing, clamped) and control points.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Curve {
    pub degree: usize,
    pub knots: Vec<f32>,
    pub points: Vec<Point>,
}

impl Curve {
    /// The index of the last control point.
    fn last(&self) -> Result<usize> {
        sub(self.points.len(), 1)
    }

    /// The index of the last knot.
    fn last_knot(&self) -> Result<usize> {
        sub(self.knots.len(), 1)
    }

    /// The knot span `u` lies in: the `i` with `knots[i] <= u < knots[i + 1]`, the last span holding the end
    /// (A2.1).
    fn span(&self, u: f32) -> Result<usize> {
        let n = self.last()?;
        let p = self.degree;
        if u == at(&self.knots, add(n, 1)?)? {
            return Ok(n);
        }
        let (mut low, mut high) = (p, add(n, 1)?);
        let mut mid = add(low, high)? / 2;
        while u < at(&self.knots, mid)? || u >= at(&self.knots, add(mid, 1)?)? {
            if u < at(&self.knots, mid)? {
                high = mid;
            } else {
                low = mid;
            }
            let next = add(low, high)? / 2;
            if next == mid {
                return Err(bug("a parameter outside the knots"));
            }
            mid = next;
        }
        Ok(mid)
    }

    /// The `degree + 1` basis functions that are not zero in span `i` at `u` (A2.2).
    fn basis(&self, i: usize, u: f32) -> Result<[f32; MAX_DEGREE + 2]> {
        let p = self.degree;
        let mut n = [0.0f32; MAX_DEGREE + 2];
        let mut left = [0.0f32; MAX_DEGREE + 2];
        let mut right = [0.0f32; MAX_DEGREE + 2];
        *slot(&mut n, 0)? = 1.0;
        for j in 1..=p {
            *slot(&mut left, j)? = u - at(&self.knots, sub(add(i, 1)?, j)?)?;
            *slot(&mut right, j)? = at(&self.knots, add(i, j)?)? - u;
            let mut saved = 0.0f32;
            for r in 0..j {
                let rr = at(&right, add(r, 1)?)?;
                let lr = at(&left, sub(j, r)?)?;
                let temp = at(&n, r)? / (rr + lr);
                *slot(&mut n, r)? = saved + rr * temp;
                saved = lr * temp;
            }
            *slot(&mut n, j)? = saved;
        }
        Ok(n)
    }

    /// The curve at `u` (A3.1).
    pub(crate) fn at(&self, u: f32) -> Result<Point> {
        let span = self.span(u)?;
        let n = self.basis(span, u)?;
        let first = sub(span, self.degree)?;
        let mut c = scaled(at(&self.points, first)?, at(&n, 0)?);
        for i in 1..=self.degree {
            c = plus(c, scaled(at(&self.points, add(first, i)?)?, at(&n, i)?));
        }
        Ok(c)
    }
}

/// Basis function `i` of degree `p` over `knots` at `u` (A2.4), the first and last being 1 at their end.
fn one_basis(knots: &[f32], p: usize, i: usize, u: f32) -> Result<f32> {
    let m = sub(knots.len(), 1)?;
    let k = |j: usize| at(knots, j);
    if (i == 0 && u == k(0)?) || (add(add(i, p)?, 1)? == m && u == k(m)?) {
        return Ok(1.0);
    }
    if u < k(i)? || u >= k(add(add(i, p)?, 1)?)? {
        return Ok(0.0);
    }
    let mut n = [0.0f32; MAX_DEGREE + 2];
    for j in 0..=p {
        *slot(&mut n, j)? = if u >= k(add(i, j)?)? && u < k(add(add(i, j)?, 1)?)? {
            1.0
        } else {
            0.0
        };
    }
    for d in 1..=p {
        let n0 = at(&n, 0)?;
        let mut saved = if n0 == 0.0 {
            0.0
        } else {
            ((u - k(i)?) * n0) / (k(add(i, d)?)? - k(i)?)
        };
        for j in 0..sub(add(p, 1)?, d)? {
            let left = k(add(add(i, j)?, 1)?)?;
            let right = k(add(add(add(i, j)?, d)?, 1)?)?;
            let next = at(&n, add(j, 1)?)?;
            if next == 0.0 {
                *slot(&mut n, j)? = saved;
                saved = 0.0;
            } else {
                let temp = next / (right - left);
                *slot(&mut n, j)? = saved + (right - u) * temp;
                saved = (u - left) * temp;
            }
        }
    }
    at(&n, 0)
}

/// Whether knot `i` is the last of its run of equal knots.
fn last_of_run(knots: &[f32], i: usize) -> bool {
    match (knots.get(i), knots.get(i.saturating_add(1))) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

/// How many knots equal knot `i`.
fn multiplicity(knots: &[f32], i: usize) -> usize {
    let Some(&value) = knots.get(i) else {
        return 0;
    };
    knots.iter().filter(|&&k| k == value).count()
}

/// The number of distinct knot values.
fn distinct(knots: &[f32]) -> usize {
    knots
        .windows(2)
        .filter(|w| w.first() != w.last())
        .count()
        .saturating_add(1)
}

/// The control points knot `r` (the last of `s` equal knots) would be replaced by if it were removed once, as the
/// two-sided pass of A5.8 computes them: the points from the left in `temp[1..]`, from the right towards the end.
struct Removal {
    temp: [Point; 2 * MAX_DEGREE + 3],
    /// The index in `temp` after the last point from the left.
    ii: usize,
    /// The index in `temp` before the first point from the right.
    jj: usize,
    /// The control point indices the passes stopped at: crossed (`j < i`) or met on one point (`j == i`).
    i: usize,
    j: usize,
    first: usize,
    last: usize,
}

fn removal(c: &Curve, r: usize, s: usize) -> Result<Removal> {
    let p = c.degree;
    let u = at(&c.knots, r)?;
    let order = add(p, 1)?;
    let last = sub(r, s)?;
    let first = sub(r, p)?;
    let off = sub(first, 1)?;
    let mut temp = [[0.0f32; 4]; 2 * MAX_DEGREE + 3];
    *slot(&mut temp, 0)? = at(&c.points, off)?;
    *slot(&mut temp, sub(add(last, 1)?, off)?)? = at(&c.points, add(last, 1)?)?;
    let (mut i, mut j) = (first, last);
    let (mut ii, mut jj) = (1usize, sub(last, off)?);
    while j > i {
        let alfi = (u - at(&c.knots, i)?) / (at(&c.knots, add(i, order)?)? - at(&c.knots, i)?);
        let alfj = (u - at(&c.knots, j)?) / (at(&c.knots, add(j, order)?)? - at(&c.knots, j)?);
        let before = at(&temp, sub(ii, 1)?)?;
        *slot(&mut temp, ii)? = divided(minus(at(&c.points, i)?, scaled(before, 1.0 - alfi)), alfi);
        let after = at(&temp, add(jj, 1)?)?;
        *slot(&mut temp, jj)? = divided(minus(at(&c.points, j)?, scaled(after, alfj)), 1.0 - alfj);
        i = add(i, 1)?;
        ii = add(ii, 1)?;
        j = sub(j, 1)?;
        jj = sub(jj, 1)?;
    }
    Ok(Removal {
        temp,
        ii,
        jj,
        i,
        j,
        first,
        last,
    })
}

/// How far the curve moves if knot `r` (the last of `s` equal ones) is removed once: the bound of equations (9.80)
/// and (9.82).
fn removal_bound(c: &Curve, r: usize, s: usize) -> Result<f32> {
    let w = removal(c, r, s)?;
    removal_distance(c, r, &w)
}

/// The distance [`removal_bound`] measures, from a two-sided pass.
fn removal_distance(c: &Curve, r: usize, w: &Removal) -> Result<f32> {
    let left = at(&w.temp, sub(w.ii, 1)?)?;
    if w.j < w.i {
        // the passes crossed: the two new points nearest the middle should coincide
        return Ok(distance(at(&w.temp, add(w.jj, 1)?)?, left));
    }
    // they met on one point: compare it with the blend of its new neighbours
    let p = c.degree;
    let u = at(&c.knots, r)?;
    let i = w.i;
    let alfi = (u - at(&c.knots, i)?) / (at(&c.knots, add(add(i, p)?, 1)?)? - at(&c.knots, i)?);
    let blend = plus(
        scaled(at(&w.temp, add(w.ii, 1)?)?, alfi),
        scaled(left, 1.0 - alfi),
    );
    Ok(distance(at(&c.points, i)?, blend))
}

/// Remove knot `r` (the last of `s` equal ones) once, whatever it costs (A5.8 for one removal).
fn remove_knot(c: &mut Curve, r: usize, s: usize) -> Result<()> {
    let p = c.degree;
    let n = c.last()?;
    let m = c.last_knot()?;
    let w = removal(c, r, s)?;
    // the removal is unconditional, but a pass that produced a non-finite point cannot be applied
    if removal_distance(c, r, &w)?.is_nan() {
        return Err(invalid(0, "a knot removal produced a point that is not finite"));
    }
    let Removal {
        temp, first, last, ..
    } = w;
    let off = sub(first, 1)?;
    let (mut i, mut j) = (first, last);
    while j > i {
        *slot(&mut c.points, i)? = at(&temp, sub(i, off)?)?;
        *slot(&mut c.points, j)? = at(&temp, sub(j, off)?)?;
        i = add(i, 1)?;
        j = sub(j, 1)?;
    }
    for k in add(r, 1)?..=m {
        *slot(&mut c.knots, sub(k, 1)?)? = at(&c.knots, k)?;
    }
    let out = sub(add(r, r)?, add(s, p)?)? / 2;
    let mut to = out;
    for k in add(out, 1)?..=n {
        *slot(&mut c.points, to)? = at(&c.points, k)?;
        to = add(to, 1)?;
    }
    c.points.truncate(n);
    c.knots.truncate(m);
    Ok(())
}

/// The smallest index `k` of the non-decreasing `values` with `values[k] >= v` (`values.len()` if none).
fn first_at_least(values: &[f32], v: f32) -> usize {
    values.partition_point(|&x| x < v)
}

/// The removal bounds of the distinct knots `lo..=hi` (each the last of its run) into `bounds`, the others there
/// infinite.
fn bounds_between(c: &Curve, bounds: &mut [f32], lo: usize, hi: usize) -> Result<()> {
    for i in lo..=hi {
        *slot(bounds, i)? = f32::INFINITY;
    }
    for i in lo..=hi {
        if last_of_run(&c.knots, i) {
            *slot(bounds, i)? = removal_bound(c, i, multiplicity(&c.knots, i))?;
        }
    }
    Ok(())
}

/// Remove, cheapest first, every interior knot whose removal keeps each sample's accumulated error (`errors`, updated)
/// within `tolerance` (A9.8 and A9.9). Returns how many were removed.
fn remove_within(c: &mut Curve, times: &[f32], errors: &mut [f32], tolerance: f32) -> Result<usize> {
    let p = c.degree;
    let before = c.points.len();
    let mut m = c.last_knot()?;
    let mut bounds = vec![f32::INFINITY; add(m, 1)?];
    let mut temp = vec![0.0f32; times.len()];
    if let (Some(lo), Some(hi)) = (p.checked_add(1), m.checked_sub(add(p, 1)?))
        && lo <= hi
    {
        bounds_between(c, &mut bounds, lo, hi)?;
    }
    loop {
        // the first knot of the smallest bound
        let mut r = 0;
        for (i, &b) in bounds.iter().enumerate() {
            if b < at(&bounds, r)? {
                r = i;
            }
        }
        let bound = at(&bounds, r)?;
        if bound == f32::INFINITY {
            break;
        }
        let s = multiplicity(&c.knots, r);
        // the one basis function the removal changes, and how much (equations 9.81 and 9.83)
        let (alpha, nk) = if add(p, s)? % 2 == 0 {
            (0.0f32, sub(r, add(p, s)? / 2)?)
        } else {
            let k = add(add(p, s)?, 1)? / 2;
            let a = at(&c.knots, sub(add(r, 1)?, k)?)?;
            let b = at(&c.knots, sub(add(add(r, p)?, 2)?, k)?)?;
            ((at(&c.knots, r)? - a) / (b - a), sub(add(r, 1)?, k)?)
        };
        let lo = first_at_least(times, at(&c.knots, nk)?);
        let hi = first_at_least(times, at(&c.knots, add(add(nk, p)?, 1)?)?);
        let mut worst = 0.0f32;
        for k in lo..hi {
            let grown = at(errors, k)? + bound * (1.0 - alpha) * one_basis(&c.knots, p, nk, at(times, k)?)?;
            *slot(&mut temp, k)? = grown;
            worst = worst.max(grown);
        }
        if worst <= tolerance {
            for k in lo..hi {
                *slot(errors, k)? = at(&temp, k)?;
            }
            remove_knot(c, r, s)?;
            m = sub(m, 1)?;
            if add(m, 1)? <= add(add(p, 1)?, add(p, 1)?)? {
                break;
            }
            bounds.remove(r);
            let n = c.last()?;
            let mut lo = std::cmp::max(sub(r, p)?, add(p, 1)?);
            let mut hi = std::cmp::min(sub(add(r, p)?, s)?, n);
            while !last_of_run(&c.knots, lo) {
                lo = add(lo, 1)?;
            }
            while !last_of_run(&c.knots, hi) {
                hi = add(hi, 1)?;
            }
            if lo <= hi {
                bounds_between(c, &mut bounds, lo, hi)?;
            }
        } else {
            *slot(&mut bounds, r)? = f32::INFINITY;
        }
    }
    sub(before, c.points.len())
}

/// Raise the multiplicity of every distinct knot by one. Returns the number of interior knots added.
fn elevate_knots(knots: &mut Vec<f32>) -> usize {
    let mut out = Vec::with_capacity(knots.len().saturating_mul(2));
    let mut previous = None;
    for &k in knots.iter() {
        if previous.is_some_and(|p: f32| p != k) {
            out.push(previous.unwrap_or(k));
        }
        out.push(k);
        previous = Some(k);
    }
    if let Some(last) = previous {
        out.push(last);
    }
    let added = out.len().saturating_sub(knots.len());
    *knots = out;
    added.saturating_sub(2)
}

/// Lower the multiplicity of every distinct knot by one (undoes [`elevate_knots`]).
fn lower_knots(knots: &mut Vec<f32>) {
    let mut out = Vec::with_capacity(knots.len());
    let mut iter = knots.iter().peekable();
    while let Some(&k) = iter.next() {
        // drop the last of each run
        if iter.peek().is_some_and(|&&next| next == k) {
            out.push(k);
        }
    }
    *knots = out;
}

/// The binomial coefficient `n choose k`, for the small `n` of degree elevation.
fn binomial(n: usize, k: usize) -> Result<f32> {
    let mut v = 1u32;
    for i in 0..k {
        let i = u32::try_from(i).map_err(|_| bug("binomial"))?;
        let n = u32::try_from(n).map_err(|_| bug("binomial"))?;
        v = v
            .checked_mul(n.checked_sub(i).ok_or_else(|| bug("binomial"))?)
            .and_then(|v| v.checked_div(i.checked_add(1)?))
            .ok_or_else(|| bug("binomial"))?;
    }
    Ok(f32::from(u16::try_from(v).map_err(|_| bug("binomial"))?))
}

/// Raise the degree of `c` by one without changing its shape (A5.9 with `t = 1`).
fn elevate_curve(c: &mut Curve) -> Result<()> {
    let p = c.degree;
    let ph = add(p, 1)?;
    if ph > MAX_DEGREE {
        return Err(bug("degree elevation past the highest degree"));
    }
    let n = c.last()?;
    let m = c.last_knot()?;
    let u = &c.knots;
    let pw = &c.points;
    let nh = sub(add(add(n, p)?, add(distinct(u), 1)?)?, add(p, 2)?)?;
    let mut uh = vec![0.0f32; add(add(nh, ph)?, 2)?];
    let mut qw = vec![[0.0f32; 4]; add(nh, 1)?];
    // the Bezier degree elevation coefficients
    let mut bez = [[0.0f32; MAX_DEGREE + 1]; MAX_DEGREE + 2];
    *slot(slot(&mut bez, 0)?, 0)? = 1.0;
    *slot(slot(&mut bez, ph)?, p)? = 1.0;
    let half = ph / 2;
    for i in 1..=half {
        let inverse = 1.0 / binomial(ph, i)?;
        for j in sub(i, 1).unwrap_or(0)..=std::cmp::min(p, i) {
            *slot(slot(&mut bez, i)?, j)? = inverse * binomial(p, j)? * binomial(1, sub(i, j)?)?;
        }
    }
    for i in add(half, 1)?..ph {
        for j in sub(i, 1).unwrap_or(0)..=std::cmp::min(p, i) {
            let mirrored = at(at(&bez, sub(ph, i)?)?.as_slice(), sub(p, j)?)?;
            *slot(slot(&mut bez, i)?, j)? = mirrored;
        }
    }
    let mut bpts = [[0.0f32; 4]; MAX_DEGREE + 2];
    let mut ebpts = [[0.0f32; 4]; MAX_DEGREE + 2];
    let mut next_bpts = [[0.0f32; 4]; MAX_DEGREE + 2];
    let mut alfs = [0.0f32; MAX_DEGREE + 2];
    let mut mh = ph;
    let mut kind = add(ph, 1)?;
    let mut r: isize = -1;
    let mut a = p;
    let mut b = add(p, 1)?;
    let mut cind = 1usize;
    let mut ua = at(u, 0)?;
    *slot(&mut qw, 0)? = at(pw, 0)?;
    for i in 0..=ph {
        *slot(&mut uh, i)? = ua;
    }
    for i in 0..=p {
        *slot(&mut bpts, i)? = at(pw, i)?;
    }
    let signed = |v: usize| isize::try_from(v).map_err(|_| bug("index"));
    let unsigned = |v: isize| usize::try_from(v).map_err(|_| bug("index"));
    while b < m {
        let i0 = b;
        while b < m && at(u, b)? == at(u, add(b, 1)?)? {
            b = add(b, 1)?;
        }
        let mul = add(sub(b, i0)?, 1)?;
        mh = add(mh, add(mul, 1)?)?;
        let ub = at(u, b)?;
        let oldr = r;
        r = isub(signed(p)?, signed(mul)?)?;
        let lbz = if oldr > 0 {
            unsigned(iadd(oldr, 2)? / 2)?
        } else {
            1
        };
        let rbz = if r > 0 {
            sub(ph, unsigned(iadd(r, 1)? / 2)?)?
        } else {
            ph
        };
        if r > 0 {
            // insert knot ub r times to get the Bezier segment
            let numer = ub - ua;
            let mut k = p;
            while k > mul {
                *slot(&mut alfs, sub(sub(k, mul)?, 1)?)? = numer / (at(u, add(a, k)?)? - ua);
                k = sub(k, 1)?;
            }
            for j in 1..=unsigned(r)? {
                let save = sub(unsigned(r)?, j)?;
                let s = add(mul, j)?;
                let mut k = p;
                while k >= s {
                    let alf = at(&alfs, sub(k, s)?)?;
                    *slot(&mut bpts, k)? = plus(
                        scaled(at(&bpts, k)?, alf),
                        scaled(at(&bpts, sub(k, 1)?)?, 1.0 - alf),
                    );
                    k = sub(k, 1)?;
                }
                *slot(&mut next_bpts, save)? = at(&bpts, p)?;
            }
        }
        // degree elevate the Bezier segment
        for i in lbz..=ph {
            let mut e = [0.0f32; 4];
            for j in sub(i, 1).unwrap_or(0)..=std::cmp::min(p, i) {
                e = plus(e, scaled(at(&bpts, j)?, at(at(&bez, i)?.as_slice(), j)?));
            }
            *slot(&mut ebpts, i)? = e;
        }
        if oldr > 1 {
            // remove knot ua oldr times
            let mut first = sub(kind, 2)?;
            let mut last = kind;
            let den = ub - ua;
            let bet = (ub - at(&uh, sub(kind, 1)?)?) / den;
            for tr in 1..oldr {
                let (mut i, mut j) = (first, last);
                let mut kj = iadd(isub(signed(j)?, signed(kind)?)?, 1)?;
                while isub(signed(j)?, signed(i)?)? > tr {
                    if i < cind {
                        let alf = (ub - at(&uh, i)?) / (ua - at(&uh, i)?);
                        let blended = plus(scaled(at(&qw, i)?, alf), scaled(at(&qw, sub(i, 1)?)?, 1.0 - alf));
                        *slot(&mut qw, i)? = blended;
                    }
                    if j >= lbz {
                        let kju = unsigned(kj)?;
                        let factor =
                            if isub(signed(j)?, tr)? <= iadd(isub(signed(kind)?, signed(ph)?)?, oldr)? {
                                (ub - at(&uh, unsigned(isub(signed(j)?, tr)?)?)?) / den
                            } else {
                                bet
                            };
                        let blended = plus(
                            scaled(at(&ebpts, kju)?, factor),
                            scaled(at(&ebpts, add(kju, 1)?)?, 1.0 - factor),
                        );
                        *slot(&mut ebpts, kju)? = blended;
                    }
                    i = add(i, 1)?;
                    j = sub(j, 1)?;
                    kj = isub(kj, 1)?;
                }
                first = sub(first, 1)?;
                last = add(last, 1)?;
            }
        }
        if a != p {
            // load the knot ua
            for _ in 0..unsigned(isub(signed(ph)?, oldr)?)? {
                *slot(&mut uh, kind)? = ua;
                kind = add(kind, 1)?;
            }
        }
        for j in lbz..=rbz {
            *slot(&mut qw, cind)? = at(&ebpts, j)?;
            cind = add(cind, 1)?;
        }
        if b < m {
            for j in 0..unsigned(r.max(0))? {
                *slot(&mut bpts, j)? = at(&next_bpts, j)?;
            }
            for j in unsigned(r.max(0))?..=p {
                *slot(&mut bpts, j)? = at(pw, sub(add(b, j)?, p)?)?;
            }
            a = b;
            b = add(b, 1)?;
            ua = ub;
        } else {
            for i in 0..=ph {
                *slot(&mut uh, add(kind, i)?)? = ub;
            }
        }
    }
    if add(nh, add(ph, 1)?)? != mh {
        return Err(bug("degree elevation counted its points wrongly"));
    }
    c.points = qw;
    c.knots = uh;
    c.degree = ph;
    Ok(())
}

/// The sparse matrix `A` of the least-squares system, `(row, column, value)` in the order they were set, and the
/// products with its normal matrix `A^T A`.
struct Normal {
    rows: usize,
    entries: Vec<(usize, usize, f32)>,
}

impl Normal {
    /// `out += A^T A x`, through `A x` first.
    fn multiply_add(&self, out: &mut [f32], x: &[f32]) -> Result<()> {
        let mut ax = vec![0.0f32; self.rows];
        for &(row, column, value) in &self.entries {
            *slot(&mut ax, row)? += value * at(x, column)?;
        }
        for &(row, column, value) in &self.entries {
            *slot(out, column)? += value * at(&ax, row)?;
        }
        Ok(())
    }
}

fn inner(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0f32, |sum, (x, y)| sum + x * y)
}

/// Solve `A^T A x = b` by conjugate gradients from the guess in `x`: at most 50 iterations, the residual recomputed
/// from scratch every 50, done when its squared length falls below `tolerance^2`.
fn conjugate_gradient(a: &Normal, x: &mut [f32], b: &[f32], tolerance: f32) -> Result<bool> {
    const ITERATIONS: usize = 50;
    let size = x.len();
    let mut product = vec![0.0f32; size];
    a.multiply_add(&mut product, x)?;
    let mut r: Vec<f32> = b.iter().zip(&product).map(|(b, p)| b - p).collect();
    let mut d = r.clone();
    let mut q = vec![0.0f32; size];
    let mut fresh = inner(&r, &r);
    let bound = tolerance * tolerance;
    let mut done = 0;
    while done < ITERATIONS {
        if fresh < bound {
            return Ok(true);
        }
        q.fill(0.0);
        a.multiply_add(&mut q, &d)?;
        let step = fresh / inner(&d, &q);
        for (x, d) in x.iter_mut().zip(&d) {
            *x += step * d;
        }
        done = add(done, 1)?;
        if done % ITERATIONS == 0 {
            for (r, b) in r.iter_mut().zip(b) {
                *r = -b;
            }
            a.multiply_add(&mut r, x)?;
            for r in &mut r {
                *r = -*r;
            }
        } else {
            for (r, q) in r.iter_mut().zip(&q) {
                *r += -step * q;
            }
        }
        let old = fresh;
        fresh = inner(&r, &r);
        let beta = fresh / old;
        for (d, r) in d.iter_mut().zip(&r) {
            *d = r + beta * *d;
        }
    }
    Ok(false)
}

/// Whether every non-empty knot span of `c` holds a sample time more than `margin` inside it.
fn well_posed(c: &Curve, times: &[f32], margin: f32) -> Result<bool> {
    let p = c.degree;
    let n = c.last()?;
    let m = c.last_knot()?;
    let mut covered = vec![false; add(n, 1)?];
    for &t in times {
        let span = c.span(t)?;
        let lo = at(&c.knots, span)? + margin;
        let hi = at(&c.knots, if span == m { m } else { add(span, 1)? })? - margin;
        if t >= lo && t <= hi {
            *slot(&mut covered, sub(span, p)?)? = true;
        }
    }
    for (i, covered) in covered.iter().enumerate() {
        if last_of_run(&c.knots, add(i, p)?) && !covered {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The least-squares control points of a curve of `c`'s degree and knots through `samples` at `times`, the end points
/// fixed at the first and last sample (equations 9.63-9.67), or `None` when the system is underdetermined, ill-posed,
/// does not converge, or the fit misses a sample by more than 1.1 times `tolerance`.
fn least_squares(c: &Curve, samples: &[Point], times: &[f32], tolerance: f32) -> Result<Option<Vec<Point>>> {
    const NUMERIC: f32 = 1e-5;
    let p = c.degree;
    let n = c.last()?;
    let m = sub(samples.len(), 1)?;
    if n > m {
        return Ok(None);
    }
    let knots = &c.knots;
    let mut entries = Vec::new();
    for i in 1..m {
        for j in 1..n {
            let v = one_basis(knots, p, j, at(times, i)?)?;
            if v != 0.0 {
                entries.push((sub(i, 1)?, sub(j, 1)?, v));
            }
        }
    }
    let a = Normal {
        rows: m.saturating_sub(1),
        entries,
    };
    let (first, final_) = (at(samples, 0)?, at(samples, m)?);
    let mut rest = vec![[0.0f32; 4]; add(m, 1)?];
    for k in 1..m {
        let t = at(times, k)?;
        let r = minus(
            minus(at(samples, k)?, scaled(first, one_basis(knots, p, 0, t)?)),
            scaled(final_, one_basis(knots, p, n, t)?),
        );
        *slot(&mut rest, k)? = r;
    }
    let unknowns = n.saturating_sub(1);
    let mut rhs = vec![[0.0f32; 4]; unknowns];
    for i in 1..n {
        let mut s = [0.0f32; 4];
        for k in 1..m {
            s = plus(s, scaled(at(&rest, k)?, one_basis(knots, p, i, at(times, k)?)?));
        }
        *slot(&mut rhs, sub(i, 1)?)? = s;
    }
    if !well_posed(c, times, NUMERIC)? {
        return Ok(None);
    }
    let mut solution = vec![[0.0f32; 4]; unknowns];
    for (i, x) in solution.iter_mut().enumerate() {
        *x = at(samples, add(i, 1)?)?;
    }
    for lane in 0..4 {
        let mut x: Vec<f32> = solution
            .iter()
            .map(|p| p.get(lane).copied().unwrap_or_default())
            .collect();
        let b: Vec<f32> = rhs
            .iter()
            .map(|p| p.get(lane).copied().unwrap_or_default())
            .collect();
        if !conjugate_gradient(&a, &mut x, &b, NUMERIC)? {
            return Ok(None);
        }
        for (p, v) in solution.iter_mut().zip(x) {
            *slot(p, lane)? = v;
        }
    }
    let mut points = Vec::with_capacity(add(n, 1)?);
    points.push(first);
    points.extend(solution);
    points.push(final_);
    let fitted = Curve {
        degree: p,
        knots: c.knots.clone(),
        points,
    };
    let limit = tolerance * 1.1;
    let mut within = true;
    for (q, &t) in samples.iter().zip(times) {
        if distance(*q, fitted.at(t)?) > limit {
            within = false;
        }
    }
    Ok(within.then_some(fitted.points))
}

/// Each sample's distance from the curve at its own time.
fn update_errors(c: &Curve, samples: &[Point], times: &[f32], errors: &mut [f32]) -> Result<()> {
    for ((e, q), &t) in errors.iter_mut().zip(samples).zip(times) {
        *e = distance(*q, c.at(t)?);
    }
    Ok(())
}

/// A curve of degree `degree` (1 to 3) within `tolerance` of every sample at its time. `times` must not decrease and
/// there must be at least two samples.
pub(crate) fn approximate(times: &[f32], samples: &[Point], degree: usize, tolerance: f32) -> Result<Curve> {
    if !(1..=MAX_DEGREE).contains(&degree) {
        return Err(invalid(
            0,
            format!("a spline of degree {degree} is not made (1 to 3)"),
        ));
    }
    if samples.len() < 2 || samples.len() != times.len() {
        return Err(bug("a fit of fewer than two samples"));
    }
    // the polyline through every sample
    let mut knots = Vec::with_capacity(add(samples.len(), 2)?);
    knots.push(at(times, 0)?);
    knots.extend_from_slice(times);
    knots.push(at(times, sub(times.len(), 1)?)?);
    let mut c = Curve {
        degree: 1,
        knots,
        points: samples.to_vec(),
    };
    let mut errors = vec![0.0f32; samples.len()];
    let mut removed = 0;
    for d in 1..=degree {
        removed = remove_within(&mut c, times, &mut errors, tolerance)?;
        if d == degree {
            break;
        }
        let saved = c.clone();
        elevate_knots(&mut c.knots);
        c.degree = add(d, 1)?;
        c.points.resize(sub(sub(c.knots.len(), c.degree)?, 1)?, [0.0; 4]);
        match least_squares(&c, samples, times, tolerance)? {
            Some(points) => c.points = points,
            None => {
                let mut lowered = c.knots.clone();
                lower_knots(&mut lowered);
                if lowered != saved.knots {
                    return Err(bug("lowering the knots did not undo raising them"));
                }
                c = saved;
                elevate_curve(&mut c)?;
            }
        }
        update_errors(&c, samples, times, &mut errors)?;
    }
    if removed == 0 {
        return Ok(c);
    }
    if let Some(points) = least_squares(&c, samples, times, tolerance)? {
        c.points = points;
    }
    update_errors(&c, samples, times, &mut errors)?;
    remove_within(&mut c, times, &mut errors, tolerance)?;
    Ok(c)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;

    /// A small deterministic generator, so the fixtures need no dependency.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
    }

    fn times(n: usize, end: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                if i + 1 == n {
                    end
                } else {
                    i as f32 / (n - 1) as f32 * end
                }
            })
            .collect()
    }

    fn cubic_curve() -> Curve {
        Curve {
            degree: 3,
            knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 2.5, 4.0, 4.0, 4.0, 4.0],
            points: vec![
                [0.0, 1.0, 2.0, 0.0],
                [1.0, -1.0, 0.5, 0.0],
                [2.0, 3.0, -1.0, 0.0],
                [0.5, 0.5, 0.5, 0.0],
                [-1.0, 2.0, 1.0, 0.0],
                [3.0, 0.0, -2.0, 0.0],
                [1.0, 1.0, 1.0, 0.0],
            ],
        }
    }

    #[test]
    fn basis_functions_sum_to_one_and_agree_with_one_basis() {
        let c = cubic_curve();
        for k in 0..=40 {
            let u = k as f32 * 0.1;
            let span = c.span(u).unwrap();
            let n = c.basis(span, u).unwrap();
            let sum: f32 = n[..=3].iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "{u}: {sum}");
            for (j, &nj) in n.iter().enumerate().take(4) {
                let one = one_basis(&c.knots, 3, span - 3 + j, u).unwrap();
                assert!((one - nj).abs() < 1e-6, "{u} {j}: {one} {nj}");
            }
        }
    }

    /// Raising the degree keeps the curve: the same values everywhere.
    #[test]
    fn degree_elevation_keeps_the_curve() {
        for (degree, knots, count) in [
            (1usize, vec![0.0, 0.0, 1.0, 2.0, 3.0, 3.0], 4usize),
            (2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 2.5, 4.0, 4.0, 4.0], 6),
        ] {
            let mut rng = Lcg(degree as u64);
            let points: Vec<Point> = (0..count)
                .map(|_| [rng.next(), rng.next(), rng.next(), rng.next()])
                .collect();
            let before = Curve {
                degree,
                knots: knots.clone(),
                points,
            };
            let mut after = before.clone();
            elevate_curve(&mut after).unwrap();
            assert_eq!(after.degree, degree + 1);
            assert_eq!(after.knots.len(), after.points.len() + after.degree + 1);
            let end = *knots.last().unwrap();
            for k in 0..=50 {
                let u = k as f32 / 50.0 * end;
                let (a, b) = (before.at(u).unwrap(), after.at(u).unwrap());
                assert!(distance(a, b) < 1e-5, "degree {degree} at {u}: {a:?} {b:?}");
            }
        }
    }

    /// A knot inserted into a curve (which leaves it unchanged) can be removed again with a bound of about zero, and
    /// the removal gives back the curve.
    #[test]
    fn an_inserted_knot_is_removed_exactly() {
        // degree 1: a straight segment with a knot in the middle
        let line = Curve {
            degree: 1,
            knots: vec![0.0, 0.0, 1.0, 2.0, 2.0],
            points: vec![[0.0, 0.0, 0.0, 0.0], [1.0, 2.0, 3.0, 0.0], [2.0, 4.0, 6.0, 0.0]],
        };
        assert!(removal_bound(&line, 2, 1).unwrap() < 1e-6);
        let mut c = line.clone();
        remove_knot(&mut c, 2, 1).unwrap();
        assert_eq!(c.knots, [0.0, 0.0, 2.0, 2.0]);
        for k in 0..=20 {
            let u = k as f32 * 0.1;
            assert!(distance(c.at(u).unwrap(), line.at(u).unwrap()) < 1e-6);
        }
        // a bent polyline: the bound is the distance of the middle point from the chord
        let bent = Curve {
            degree: 1,
            knots: vec![0.0, 0.0, 1.0, 2.0, 2.0],
            points: vec![[0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]],
        };
        assert!((removal_bound(&bent, 2, 1).unwrap() - 1.0).abs() < 1e-6);
    }

    /// Every sample lies within the tolerance of the fitted curve at its own time, at every degree.
    #[test]
    fn a_fit_stays_within_its_tolerance() {
        for seed in 0..20u64 {
            let mut rng = Lcg(seed);
            let n = 10 + (seed as usize * 13) % 240;
            let t = times(n, 3.0);
            // a smooth curve plus noise
            let samples: Vec<Point> = t
                .iter()
                .map(|&x| {
                    [
                        (x * 2.0).sin() + rng.next() * 0.002,
                        (x * 0.7).cos() * 3.0,
                        x * x * 0.1 + rng.next() * 0.01,
                        0.0,
                    ]
                })
                .collect();
            for degree in 1..=3 {
                let tolerance = 0.001;
                let c = approximate(&t, &samples, degree, tolerance).unwrap();
                assert_eq!(c.degree, degree);
                assert_eq!(c.knots.len(), c.points.len() + degree + 1);
                let worst = samples
                    .iter()
                    .zip(&t)
                    .map(|(q, &u)| distance(*q, c.at(u).unwrap()))
                    .fold(0.0f32, f32::max);
                assert!(worst <= tolerance * 1.1, "seed {seed} degree {degree}: {worst}");
            }
        }
    }
}
