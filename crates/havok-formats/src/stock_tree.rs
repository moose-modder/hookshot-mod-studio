//! The tree the stock tool builds over the primitives of a mesh, and how it cuts that tree into sections. The sections'
//! trees and the mesh tree of a compressed mesh are pieces of this one tree ([`crate::bvtree_write`] has what the files
//! show of that). The rule below was read off the stock meshes and tested against them (see the engine wiki's page on
//! Havok collision for the census); it is not the stock tool in every detail, because the stock tool worked from the
//! vertices before they were quantised and the files keep only the quantised ones.
//!
//! 1. *Insertion.* The leaves (a box each, in the order given) are put into a binary tree one at a time. A leaf descends
//!    from the root, growing each box it passes to hold it, into the child with the smaller product of two numbers: the
//!    squared distance between the centres of that child's box and the leaf's box, and the sum of the extents of both
//!    boxes along the three axes (the second child wins only when its product is smaller). Where it stops at a leaf, the
//!    two become the children of a new node.
//! 2. *Rebuild* (with at least three leaves). The leaves are listed left to right and the tree is built again from the
//!    list. A list of more than 16 leaves (and the whole list, whatever its length) is split: its leaves are put into 32
//!    bins on each axis by the centre of their boxes (the centre's place in the list's box, times 32, plus one half,
//!    truncated and kept within 0 to 31), and each bin holds the union of its boxes and its count. On each axis, a bin
//!    costs the surface of the union of the bins up to it times their count, plus the surface of the union of the bins from
//!    it on times their count. The bin of least cost over the three axes (the first on a tie; axes in order, bins from the
//!    lowest) is the split: the bins before it make the first list, the bins after it the second, each in the order of the
//!    leaves. The leaves of the bin itself go to the side that is empty if exactly one is, and otherwise are put in
//!    order of the centre of their boxes on that axis (with the stock quicksort, [`game_sort_by`]) and divided at half,
//!    the lower half to the first list. A list of 16 or fewer is built from the bottom: the two entries whose union has
//!    the least surface (the first pair on a tie, in list order) become the children of a new node that takes the first
//!    one's place, the last entry fills the second one's, and so on until one is left.
//! 3. *Refinement.* Every node below the root whose two children both have children is looked at, in pre-order: the two
//!    ways of exchanging a grandchild between the children (the first child keeping its first grandchild and taking the
//!    other child's first or second; the other child getting the remaining two) are costed by the sum of the surfaces of
//!    the two new children, and the cheaper way (the second when equal) is taken if it costs less than the present sum.
//!    The passes repeat until one changes nothing. Then every node below the root has its children put with the one of
//!    the larger surface first.
//! 4. *The cut.* A queue starts with the root. An entry with fewer leaves than a section can hold, whose leaves the caller
//!    accepts (they fit a section's vertex slots), is a section. Any other entry is replaced by its first child, and its
//!    second child goes to the end of the queue. The sections are numbered in queue order.
//!
//! A surface here is `ez*ey + ey*ex + ex*ez` of the extents, in single precision; the whole thing is single-precision
//! arithmetic in the order written.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::bvtree_write::Shape;
use crate::gamesort::game_sort_by;

/// A box, minimum then maximum (xyz), in single precision.
pub type Box3 = [[f32; 3]; 2];

/// An empty box's bound: a value just under the largest finite `f32`.
const BIG: f32 = f32::from_bits(0x7f7f_ffee);
/// The bins on each axis of a split.
const BINS: usize = 32;
/// The last bin.
const LAST_BIN: usize = BINS - 1;
/// A list of this many leaves or fewer is built from the bottom.
const BOTTOM_UP: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Node {
    bounds: Box3,
    parent: usize,
    children: [usize; 2],
    /// The leaf's index among the boxes given, for a leaf.
    leaf: usize,
}

/// The primitive tree of a mesh: a binary tree over boxes, built by [`StockTree::build`].
#[derive(Debug, Clone)]
pub struct StockTree {
    /// Node 0 is a null node; the others are leaves and inner nodes.
    nodes: Vec<Node>,
    root: usize,
    leaf_count: usize,
}

fn lo_of(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

fn hi_of(a: f32, b: f32) -> f32 {
    if b > a { b } else { a }
}

fn union(a: &Box3, b: &Box3) -> Box3 {
    [
        [
            lo_of(a[0][0], b[0][0]),
            lo_of(a[0][1], b[0][1]),
            lo_of(a[0][2], b[0][2]),
        ],
        [
            hi_of(a[1][0], b[1][0]),
            hi_of(a[1][1], b[1][1]),
            hi_of(a[1][2], b[1][2]),
        ],
    ]
}

/// The box that holds nothing: the identity of [`union`].
const EMPTY: Box3 = [[BIG; 3], [-BIG; 3]];

/// The surface measure of a box.
fn surface(b: &Box3) -> f32 {
    let ex = b[1][0] - b[0][0];
    let ey = b[1][1] - b[0][1];
    let ez = b[1][2] - b[0][2];
    (ez * ey + ey * ex) + ex * ez
}

/// The sum of the surfaces of two boxes, added in the order the stock tool does.
fn surface_pair(a: &Box3, b: &Box3) -> f32 {
    let t = |x: &Box3| {
        let ex = x[1][0] - x[0][0];
        let ey = x[1][1] - x[0][1];
        let ez = x[1][2] - x[0][2];
        [ez * ey, ey * ex, ex * ez]
    };
    let (p, q) = (t(a), t(b));
    ((((p[0] + p[1]) + p[2]) + q[0]) + q[1]) + q[2]
}

fn lane(b: &[f32; 3], axis: usize) -> f32 {
    b.get(axis).copied().unwrap_or(0.0)
}

/// The bin of a centre: place in the list's box times the bin count, plus one half, kept within the bins.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=31 first"
)]
fn bin_of(centre: f32, low: f32, scale: f32) -> usize {
    let t = (((centre - low) * scale) * (BINS as f32) + 0.5f32)
        .max(0.0)
        .min(LAST_BIN as f32);
    t as usize
}

#[derive(Clone)]
struct Bin {
    leaves: Vec<usize>,
    bounds: Box3,
    cost: f32,
}

impl StockTree {
    /// Build the tree over `boxes` (the leaf `i` has box `boxes[i]`; they are inserted in that order).
    pub fn build(boxes: &[Box3]) -> Self {
        let mut tree = Self {
            nodes: vec![Node {
                bounds: EMPTY,
                parent: 0,
                children: [0, 0],
                leaf: usize::MAX,
            }],
            root: 0,
            leaf_count: 0,
        };
        let ids: Vec<usize> = boxes
            .iter()
            .enumerate()
            .map(|(leaf, b)| {
                tree.nodes.push(Node {
                    bounds: *b,
                    parent: 0,
                    children: [0, 0],
                    leaf,
                });
                tree.nodes.len().saturating_sub(1)
            })
            .collect();
        for &id in &ids {
            tree.insert(id);
        }
        if tree.leaf_count >= 3 {
            let flat = tree.leaves_from(tree.root);
            let root = tree.rebuild(flat, true);
            tree.root = root;
            if let Some(n) = tree.nodes.get_mut(root) {
                n.parent = 0;
            }
            tree.refine();
        }
        tree
    }

    fn node(&self, i: usize) -> Node {
        self.nodes.get(i).copied().unwrap_or(Node {
            bounds: EMPTY,
            parent: 0,
            children: [0, 0],
            leaf: usize::MAX,
        })
    }

    fn set_parent(&mut self, i: usize, parent: usize) {
        if let Some(n) = self.nodes.get_mut(i) {
            n.parent = parent;
        }
    }

    fn set_child(&mut self, i: usize, slot: usize, child: usize) {
        if let Some(c) = self.nodes.get_mut(i).and_then(|n| n.children.get_mut(slot)) {
            *c = child;
        }
    }

    fn is_inner(&self, i: usize) -> bool {
        self.node(i).children[0] != 0
    }

    fn alloc(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len().saturating_sub(1)
    }

    /// Insert the leaf node `leaf` by the descent rule.
    fn insert(&mut self, leaf: usize) {
        self.leaf_count = self.leaf_count.saturating_add(1);
        if self.root == 0 {
            self.root = leaf;
            self.set_parent(leaf, 0);
            return;
        }
        let nb = self.node(leaf).bounds;
        let sum = [nb[1][0] + nb[0][0], nb[1][1] + nb[0][1], nb[1][2] + nb[0][2]];
        let ext = [nb[1][0] - nb[0][0], nb[1][1] - nb[0][1], nb[1][2] - nb[0][2]];
        let mut current = self.root;
        while self.is_inner(current) {
            let grown = union(&self.node(current).bounds, &nb);
            if let Some(n) = self.nodes.get_mut(current) {
                n.bounds = grown;
            }
            let metric = |t: &Self, x: usize| -> f32 {
                let b = t.node(x).bounds;
                let dx = (b[1][0] + b[0][0]) - sum[0];
                let dy = (b[1][1] + b[0][1]) - sum[1];
                let dz = (b[1][2] + b[0][2]) - sum[2];
                let distance = (dy * dy + dx * dx) + dz * dz;
                let extents = (((((b[1][1] - b[0][1]) + ext[1]) + (b[1][0] - b[0][0])) + ext[0])
                    + (b[1][2] - b[0][2]))
                    + ext[2];
                distance * extents
            };
            let [a, b] = self.node(current).children;
            current = if metric(self, b) < metric(self, a) { b } else { a };
        }
        let at = self.node(current);
        let joined = union(&at.bounds, &nb);
        let inner = self.alloc(Node {
            bounds: joined,
            parent: at.parent,
            children: [current, leaf],
            leaf: usize::MAX,
        });
        if at.parent == 0 {
            self.root = inner;
        } else {
            let slot = usize::from(self.node(at.parent).children[1] == current);
            self.set_child(at.parent, slot, inner);
        }
        self.set_parent(current, inner);
        self.set_parent(leaf, inner);
    }

    /// The leaf nodes under `from`, left to right.
    fn leaves_from(&self, from: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = Vec::new();
        let mut node = from;
        loop {
            while self.is_inner(node) {
                let [first, second] = self.node(node).children;
                stack.push(second);
                node = first;
            }
            out.push(node);
            match stack.pop() {
                Some(next) => node = next,
                None => break,
            }
        }
        out
    }

    /// Build a subtree from the list of nodes (leaves or built subtrees) and return its root.
    fn rebuild(&mut self, list: Vec<usize>, is_root: bool) -> usize {
        if list.len() <= BOTTOM_UP && !is_root {
            return self.bottom_up(list);
        }
        if let [only] = list.as_slice() {
            return *only;
        }
        let whole = list
            .iter()
            .fold(EMPTY, |acc, &x| union(&acc, &self.node(x).bounds));
        let scale = [0usize, 1, 2].map(|axis| {
            let extent = lane(&whole[1], axis) - lane(&whole[0], axis);
            if extent == 0.0 { 0.0 } else { 1.0f32 / extent }
        });
        let blank = Bin {
            leaves: Vec::new(),
            bounds: EMPTY,
            cost: 0.0,
        };
        let mut bins: [Vec<Bin>; 3] = [
            vec![blank.clone(); BINS],
            vec![blank.clone(); BINS],
            vec![blank; BINS],
        ];
        let mut first_bin = [BINS; 3];
        let mut last_bin = [0usize; 3];
        for &x in &list {
            let b = self.node(x).bounds;
            for (axis, axis_bins) in bins.iter_mut().enumerate() {
                let centre = (lane(&b[1], axis) + lane(&b[0], axis)) * 0.5f32;
                let at = bin_of(
                    centre,
                    lane(&whole[0], axis),
                    scale.get(axis).copied().unwrap_or(0.0),
                );
                if let Some(slot) = first_bin.get_mut(axis) {
                    *slot = (*slot).min(at);
                }
                if let Some(slot) = last_bin.get_mut(axis) {
                    *slot = (*slot).max(at);
                }
                if let Some(bin) = axis_bins.get_mut(at) {
                    bin.bounds = union(&bin.bounds, &b);
                    bin.leaves.push(x);
                }
            }
        }
        // the cost of splitting at each occupied bin
        for (axis, axis_bins) in bins.iter_mut().enumerate() {
            let lo = first_bin.get(axis).copied().unwrap_or(0);
            let hi = last_bin.get(axis).copied().unwrap_or(0);
            let mut forward = (EMPTY, 0usize);
            let mut backward = (EMPTY, 0usize);
            let steps = hi.saturating_sub(lo);
            for step in 0..=steps {
                let up = lo.saturating_add(step);
                let down = hi.saturating_sub(step);
                if let Some(bin) = axis_bins.get(up) {
                    forward = (
                        union(&forward.0, &bin.bounds),
                        forward.1.saturating_add(bin.leaves.len()),
                    );
                }
                if let Some(bin) = axis_bins.get(down) {
                    backward = (
                        union(&backward.0, &bin.bounds),
                        backward.1.saturating_add(bin.leaves.len()),
                    );
                }
                let cost_up = surface(&forward.0) * (forward.1 as f32);
                if let Some(bin) = axis_bins.get_mut(up) {
                    bin.cost += cost_up;
                }
                let cost_down = surface(&backward.0) * (backward.1 as f32);
                if let Some(bin) = axis_bins.get_mut(down) {
                    bin.cost += cost_down;
                }
            }
        }
        let mut best = BIG;
        let mut choice: Option<(usize, usize)> = None;
        for (axis, axis_bins) in bins.iter().enumerate() {
            let lo = first_bin.get(axis).copied().unwrap_or(0);
            let hi = last_bin.get(axis).copied().unwrap_or(0);
            for at in lo..=hi {
                if let Some(bin) = axis_bins.get(at)
                    && !bin.leaves.is_empty()
                    && bin.cost < best
                {
                    best = bin.cost;
                    choice = Some((axis, at));
                }
            }
        }
        let (mut first, mut second): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        match choice.and_then(|(axis, at)| bins.get(axis).map(|b| (axis, at, b))) {
            None => {
                let half = list.len() >> 1;
                first.extend(list.iter().take(half));
                second.extend(list.iter().skip(half));
            }
            Some((axis, at, axis_bins)) => {
                for bin in axis_bins.iter().take(at) {
                    first.extend_from_slice(&bin.leaves);
                }
                for bin in axis_bins.iter().skip(at.saturating_add(1)) {
                    second.extend_from_slice(&bin.leaves);
                }
                let middle = axis_bins.get(at).map(|b| b.leaves.clone()).unwrap_or_default();
                if first.is_empty() != second.is_empty() {
                    if second.is_empty() {
                        second.extend_from_slice(&middle);
                    } else {
                        first.extend_from_slice(&middle);
                    }
                } else {
                    let mut keyed: Vec<(usize, f32)> = middle
                        .iter()
                        .map(|&x| {
                            let b = self.node(x).bounds;
                            (x, (lane(&b[1], axis) + lane(&b[0], axis)) * 0.5f32)
                        })
                        .collect();
                    game_sort_by(&mut keyed, &|p, q| p.1 < q.1);
                    let half = keyed.len() >> 1;
                    first.extend(keyed.iter().take(half).map(|p| p.0));
                    second.extend(keyed.iter().skip(half).map(|p| p.0));
                }
            }
        }
        if first.is_empty() || second.is_empty() {
            let all: Vec<usize> = first.into_iter().chain(second).collect();
            let half = all.len() >> 1;
            first = all.iter().take(half).copied().collect();
            second = all.iter().skip(half).copied().collect();
        }
        let a = self.rebuild(first, false);
        let b = self.rebuild(second, false);
        let node = self.alloc(Node {
            bounds: whole,
            parent: 0,
            children: [a, b],
            leaf: usize::MAX,
        });
        self.set_parent(a, node);
        self.set_parent(b, node);
        node
    }

    /// Join the entries of `list` pair by pair, the pair of least union surface first.
    fn bottom_up(&mut self, mut list: Vec<usize>) -> usize {
        while list.len() > 1 {
            let mut best = BIG;
            let mut pick: Option<(usize, usize, Box3)> = None;
            for (i, &a) in list.iter().enumerate() {
                for (j, &b) in list.iter().enumerate().skip(i.saturating_add(1)) {
                    let joined = union(&self.node(a).bounds, &self.node(b).bounds);
                    let cost = surface(&joined);
                    if cost < best {
                        best = cost;
                        pick = Some((i, j, joined));
                    }
                }
            }
            let (i, j, joined) = pick.unwrap_or_else(|| {
                let a = list.first().copied().unwrap_or(0);
                let b = list.get(1).copied().unwrap_or(0);
                (0, 1, union(&self.node(a).bounds, &self.node(b).bounds))
            });
            let a = list.get(i).copied().unwrap_or(0);
            let b = list.get(j).copied().unwrap_or(0);
            let node = self.alloc(Node {
                bounds: joined,
                parent: 0,
                children: [a, b],
                leaf: usize::MAX,
            });
            self.set_parent(a, node);
            self.set_parent(b, node);
            let last = list.pop();
            if j < list.len()
                && let (Some(slot), Some(last)) = (list.get_mut(j), last)
            {
                *slot = last;
            }
            if let Some(slot) = list.get_mut(i) {
                *slot = node;
            }
        }
        list.first().copied().unwrap_or(0)
    }

    /// The node after `node` in pre-order below the root, or 0 at the end.
    fn after(&self, node: usize) -> usize {
        if self.is_inner(node) {
            return self.node(node).children[0];
        }
        let mut current = node;
        let mut above = self.node(node).parent;
        while above != self.root && above != 0 && self.node(above).children[1] == current {
            current = above;
            above = self.node(above).parent;
        }
        let next = if above != 0 {
            self.node(above).children[1]
        } else {
            current
        };
        if above == self.root && next == current {
            0
        } else {
            next
        }
    }

    fn refine(&mut self) {
        loop {
            let mut changed = false;
            let mut node = self.node(self.root).children[0];
            while node != 0 {
                if self.is_inner(node) {
                    changed |= self.rotate(node);
                }
                node = self.after(node);
            }
            if !changed {
                break;
            }
        }
        let mut node = self.node(self.root).children[0];
        while node != 0 {
            if self.is_inner(node) {
                let [a, b] = self.node(node).children;
                if surface(&self.node(a).bounds) < surface(&self.node(b).bounds) {
                    self.set_child(node, 0, b);
                    self.set_child(node, 1, a);
                }
            }
            node = self.after(node);
        }
    }

    /// Exchange a grandchild of `node` between its children when that lowers the surfaces; whether it did.
    fn rotate(&mut self, node: usize) -> bool {
        let [c0, c1] = self.node(node).children;
        if !(self.is_inner(c0) && self.is_inner(c1)) {
            return false;
        }
        let [a0, a1] = self.node(c0).children;
        let [b0, b1] = self.node(c1).children;
        let (ba0, ba1, bb0, bb1) = (
            self.node(a0).bounds,
            self.node(a1).bounds,
            self.node(b0).bounds,
            self.node(b1).bounds,
        );
        let first = (union(&ba0, &bb0), union(&ba1, &bb1));
        let second = (union(&ba0, &bb1), union(&ba1, &bb0));
        let (cost_first, cost_second) = (
            surface_pair(&first.0, &first.1),
            surface_pair(&second.0, &second.1),
        );
        let use_second = cost_second <= cost_first;
        let chosen = if use_second { cost_second } else { cost_first };
        let current = surface_pair(&self.node(c0).bounds, &self.node(c1).bounds);
        if current <= chosen {
            return false;
        }
        let (bounds, moved) = if use_second {
            (second, (b1, a1, b0))
        } else {
            (first, (b0, a1, b1))
        };
        self.set_child(c0, 1, moved.0);
        self.set_child(c1, 0, moved.1);
        self.set_child(c1, 1, moved.2);
        self.set_parent(moved.0, c0);
        self.set_parent(moved.1, c1);
        self.set_parent(moved.2, c1);
        if let Some(n) = self.nodes.get_mut(c0) {
            n.bounds = bounds.0;
        }
        if let Some(n) = self.nodes.get_mut(c1) {
            n.bounds = bounds.1;
        }
        true
    }

    /// The root node.
    pub fn root(&self) -> usize {
        self.root
    }

    /// The two children of an inner node, `None` for a leaf.
    pub fn children(&self, node: usize) -> Option<[usize; 2]> {
        self.is_inner(node).then(|| self.node(node).children)
    }

    /// The index among the boxes given of the leaf `node`, `None` for an inner node.
    pub fn leaf(&self, node: usize) -> Option<usize> {
        (!self.is_inner(node)).then(|| self.node(node).leaf)
    }

    /// The box of `node`.
    pub fn bounds(&self, node: usize) -> Box3 {
        self.node(node).bounds
    }

    /// The indices of the leaves under `node`, left to right.
    pub fn leaves_under(&self, node: usize) -> Vec<usize> {
        self.leaves_from(node)
            .into_iter()
            .map(|x| self.node(x).leaf)
            .collect()
    }

    /// Cut the tree into sections: a queue starting with the root, whose entry is a section when it has at most
    /// `max_leaves` leaves and `fits` accepts its leaves (given by index, left to right); any other entry is replaced by
    /// its first child and its second child is appended. Returns the section nodes in queue order. A leaf that `fits`
    /// refuses is a section of its own.
    pub fn cut(&self, max_leaves: usize, fits: &dyn Fn(&[usize]) -> bool) -> Vec<usize> {
        let mut items = vec![self.root];
        let mut at = 0usize;
        while let Some(&node) = items.get(at) {
            let leaves = self.leaves_under(node);
            if (leaves.len() <= max_leaves && fits(&leaves)) || !self.is_inner(node) {
                at = at.saturating_add(1);
                continue;
            }
            let [first, second] = self.node(node).children;
            if let Some(slot) = items.get_mut(at) {
                *slot = first;
            }
            items.push(second);
        }
        items
    }

    /// The shape of the subtree under `node` as the writers' trees take it, with the leaves' payloads numbered in
    /// pre-order from 0, and the index among the boxes given of the leaf of each payload.
    pub fn shape_under(&self, node: usize) -> (Vec<Shape>, Vec<usize>) {
        let mut shape = Vec::new();
        let mut order = Vec::new();
        self.shape_into(
            node,
            &|_, leaf, order: &mut Vec<usize>| {
                order.push(leaf);
                u32::try_from(order.len().saturating_sub(1)).unwrap_or(u32::MAX)
            },
            &mut shape,
            &mut order,
            &[],
        );
        (shape, order)
    }

    /// The shape of the tree above the sections `items` (as [`cut`](Self::cut) returns them): each section a leaf whose
    /// payload is its index in `items`.
    pub fn shape_above(&self, items: &[usize]) -> Vec<Shape> {
        let mut shape = Vec::new();
        let mut order = Vec::new();
        self.shape_into(
            self.root,
            &|node, _, _: &mut Vec<usize>| {
                u32::try_from(items.iter().position(|&x| x == node).unwrap_or(0)).unwrap_or(u32::MAX)
            },
            &mut shape,
            &mut order,
            items,
        );
        shape
    }

    fn shape_into(
        &self,
        node: usize,
        payload: &dyn Fn(usize, usize, &mut Vec<usize>) -> u32,
        shape: &mut Vec<Shape>,
        order: &mut Vec<usize>,
        stop_at: &[usize],
    ) {
        if !self.is_inner(node) || stop_at.contains(&node) {
            let value = payload(node, self.node(node).leaf, order);
            shape.push(Shape::Leaf { payload: value });
            return;
        }
        let here = shape.len();
        shape.push(Shape::Internal { second_child: 0 });
        let [first, second] = self.node(node).children;
        self.shape_into(first, payload, shape, order, stop_at);
        let second_at = shape.len();
        if let Some(slot) = shape.get_mut(here) {
            *slot = Shape::Internal {
                second_child: second_at,
            };
        }
        self.shape_into(second, payload, shape, order, stop_at);
    }
}

/// The order in which a section holds its primitives, as the stock tool leaves it. The primitives are first numbered
/// in the pre-order of the section's tree and are then sorted, with the stock quicksort ([`game_sort_by`]), by the index of
/// their data run; a section names one data run, so every key is equal and what the sort does is a permutation that
/// depends on the count alone. The result is `ranks`, where `ranks[p]` is the pre-order rank of the primitive stored at
/// index `p`; the leaf of pre-order rank `r` carries the payload `p` for which `ranks[p] == r`. Every section tree of
/// both installs follows it (see the engine wiki's page on Havok collision for the census).
pub fn section_numbering(count: usize) -> Vec<usize> {
    let mut ranks: Vec<usize> = (0..count).collect();
    game_sort_by(&mut ranks, &|_, _| false);
    ranks
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test reads what it just built"
)]
mod tests {
    use super::*;

    fn cube(x: f32, y: f32, z: f32) -> Box3 {
        [[x, y, z], [x + 1.0, y + 1.0, z + 1.0]]
    }

    /// A row of cubes along x with a gap, so that no two boxes touch.
    fn row(n: usize) -> Vec<Box3> {
        (0..n).map(|i| cube(i as f32 * 2.0, 0.0, 0.0)).collect()
    }

    fn leaves_once(tree: &StockTree, n: usize) {
        let mut seen = tree.leaves_under(tree.root());
        seen.sort_unstable();
        assert!(seen.iter().copied().eq(0..n));
    }

    #[test]
    fn one_and_two_leaves_stay_as_inserted() {
        let one = StockTree::build(&row(1));
        assert_eq!(one.shape_under(one.root()).0, [Shape::Leaf { payload: 0 }]);
        let two = StockTree::build(&row(2));
        let (shape, order) = two.shape_under(two.root());
        assert_eq!(
            shape,
            [
                Shape::Internal { second_child: 2 },
                Shape::Leaf { payload: 0 },
                Shape::Leaf { payload: 1 }
            ]
        );
        assert_eq!(order, [0, 1]);
    }

    #[test]
    fn every_leaf_is_in_the_tree_once_whatever_its_size() {
        for n in [3usize, 4, 5, 16, 17, 33, 100, 300] {
            let tree = StockTree::build(&row(n));
            leaves_once(&tree, n);
            let (shape, order) = tree.shape_under(tree.root());
            assert_eq!(shape.len(), n * 2 - 1);
            assert_eq!(order.len(), n);
        }
    }

    #[test]
    fn a_node_holds_its_children_and_a_larger_child_comes_first_below_the_root() {
        let boxes: Vec<Box3> = (0..60)
            .map(|i| {
                let s = 1.0 + (i % 5) as f32;
                [[i as f32 * 3.0, 0.0, 0.0], [i as f32 * 3.0 + s, s, 1.0]]
            })
            .collect();
        let tree = StockTree::build(&boxes);
        leaves_once(&tree, boxes.len());
        let mut stack = vec![tree.root()];
        while let Some(n) = stack.pop() {
            if let Some([a, b]) = tree.children(n) {
                let (outer, ba, bb) = (tree.bounds(n), tree.bounds(a), tree.bounds(b));
                assert_eq!(
                    union(&ba, &bb),
                    union(&outer, &union(&ba, &bb)),
                    "a node holds its children"
                );
                if n != tree.root() {
                    assert!(surface(&ba) >= surface(&bb), "the larger child first");
                }
                stack.push(a);
                stack.push(b);
            }
        }
    }

    #[test]
    fn the_cut_makes_sections_that_hold_every_leaf_once() {
        let n = 1000;
        let tree = StockTree::build(&row(n));
        let items = tree.cut(127, &|leaves| leaves.len() <= 127);
        assert!(items.len() >= n / 127);
        let mut all: Vec<usize> = items.iter().flat_map(|&x| tree.leaves_under(x)).collect();
        assert!(items.iter().all(|&x| tree.leaves_under(x).len() <= 127));
        all.sort_unstable();
        assert!(all.iter().copied().eq(0..n));
        // the shape above the sections has one leaf per section, numbered by queue position
        let above = tree.shape_above(&items);
        let mut payloads: Vec<u32> = above
            .iter()
            .filter_map(|s| match s {
                Shape::Leaf { payload } => Some(*payload),
                Shape::Internal { .. } => None,
            })
            .collect();
        payloads.sort_unstable();
        assert!(
            payloads
                .iter()
                .copied()
                .eq(0..u32::try_from(items.len()).unwrap())
        );
    }

    #[test]
    fn a_sections_numbering_is_the_sort_of_equal_keys() {
        // read off stock section trees: the payloads of their leaves in pre-order
        assert_eq!(section_numbering(4), [2, 3, 0, 1]);
        assert_eq!(section_numbering(6), [3, 4, 5, 0, 1, 2]);
        assert_eq!(section_numbering(8), [5, 4, 7, 6, 1, 0, 3, 2]);
        assert_eq!(section_numbering(10), [6, 5, 7, 9, 8, 1, 0, 2, 4, 3]);
        for n in 0..=127 {
            let mut ranks = section_numbering(n);
            ranks.sort_unstable();
            assert!(ranks.iter().copied().eq(0..n));
        }
    }

    #[test]
    fn the_queue_replaces_a_split_entry_by_its_first_child_and_appends_the_second() {
        let tree = StockTree::build(&row(8));
        // sections of at most 2 leaves: a three-level queue
        let items = tree.cut(2, &|_| true);
        assert!(items.iter().all(|&x| tree.leaves_under(x).len() <= 2));
        let first_leaf_of_first = tree.leaves_under(items[0]);
        let root_first = tree.children(tree.root()).unwrap()[0];
        assert!(tree.leaves_under(root_first).starts_with(&first_leaf_of_first));
    }
}
