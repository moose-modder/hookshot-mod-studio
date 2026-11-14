//! The name dictionaries of the game's files (`ResDic`: a Patricia trie over the names' bits), built for a list of
//! names: what a file with added names (a skeleton with new bones) needs. Ported from Syroot.NintenTools.Bfres's
//! `ResDict.cs` as maintained in LegacySwitchLibraries (MIT).
//!
//! A dictionary is a root node, then a node per name in the names' order (so name `i` is node `i + 1`), each with
//! the bit it tests and the nodes to go to on a 0 and on a 1. Bits count from the last byte of a name, the lowest
//! bit first (bit `b` is bit `b % 8` of the byte `b / 8` from the end; past the start of the name, 0), as the game
//! reads them. The root tests no bit (`0xFFFFFFFF`).

/// One node: the bit it tests (`0xFFFFFFFF` for the root), and where a 0 and a 1 lead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DictNode {
    pub reference: u32,
    pub left: u16,
    pub right: u16,
}

/// Bit `b` of `key`, counted from its end.
fn bit(key: &[u8], b: i32) -> usize {
    if b < 0 {
        return 0;
    }
    let (byte, shift) = ((b / 8) as usize, b % 8);
    if byte >= key.len() {
        return 0;
    }
    ((key[key.len() - 1 - byte] >> shift) & 1) as usize
}

/// The lowest bit where `a` and `b` differ (`-1`: none).
fn mismatch(a: &[u8], b: &[u8]) -> i32 {
    let bits = (a.len().max(b.len()) * 8) as i32;
    (0..bits).find(|&i| bit(a, i) != bit(b, i)).unwrap_or(-1)
}

/// The lowest bit set in `key` (`-1`: none).
fn first_one(key: &[u8]) -> i32 {
    (0..(key.len() * 8) as i32)
        .find(|&i| bit(key, i) == 1)
        .unwrap_or(-1)
}

struct Node<'a> {
    key: &'a [u8],
    bit: i32,
    parent: usize,
    child: [usize; 2],
}

/// The dictionary of `names`, in their order: the root, then a node per name.
pub fn build(names: &[&str]) -> Vec<DictNode> {
    const ROOT: usize = 0;
    let mut nodes = vec![Node {
        key: &[],
        bit: -1,
        parent: ROOT,
        child: [ROOT, ROOT],
    }];
    for name in names {
        let data = name.as_bytes();
        let new = nodes.len();
        // the last node on the way down for this key
        let mut current = if nodes[ROOT].child[0] == ROOT {
            ROOT
        } else {
            let mut node = nodes[ROOT].child[0];
            let mut prev;
            loop {
                prev = node;
                node = nodes[node].child[bit(data, nodes[node].bit)];
                if nodes[node].bit <= nodes[prev].bit {
                    break;
                }
            }
            prev
        };
        let at = mismatch(nodes[current].key, data);
        while at < nodes[nodes[current].parent].bit {
            current = nodes[current].parent;
        }
        let cur_bit = nodes[current].bit;
        if at < cur_bit {
            let parent = nodes[current].parent;
            let mut child = [new, new];
            child[bit(data, at) ^ 1] = current;
            nodes.push(Node {
                key: data,
                bit: at,
                parent,
                child,
            });
            let side = bit(data, nodes[parent].bit);
            nodes[parent].child[side] = new;
            nodes[current].parent = new;
        } else if at > cur_bit {
            let mut child = [new, new];
            child[bit(data, at) ^ 1] = if bit(nodes[current].key, at) == bit(data, at) ^ 1 {
                current
            } else {
                ROOT
            };
            nodes.push(Node {
                key: data,
                bit: at,
                parent: current,
                child,
            });
            let side = bit(data, cur_bit);
            nodes[current].child[side] = new;
        } else {
            let side = bit(data, at);
            let next = nodes[current].child[side];
            let new_bit = if next != ROOT {
                mismatch(nodes[next].key, data)
            } else {
                first_one(data)
            };
            let mut child = [new, new];
            child[bit(data, new_bit) ^ 1] = next;
            nodes.push(Node {
                key: data,
                bit: new_bit,
                parent: current,
                child,
            });
            nodes[current].child[side] = new;
        }
    }
    nodes
        .iter()
        .map(|n| DictNode {
            reference: n.bit as u32,
            left: n.child[0] as u16,
            right: n.child[1] as u16,
        })
        .collect()
}

/// The index in `names` of `key`, looked up in a dictionary of them (`nodes` as [`build`] makes them) as the game
/// looks a name up: down the nodes while the bit tested rises, then the name of the node reached compared.
pub fn find(nodes: &[DictNode], names: &[&str], key: &str) -> Option<usize> {
    let data = key.as_bytes();
    let mut prev = *nodes.first()?;
    let mut index = prev.left as usize;
    loop {
        let node = *nodes.get(index)?;
        if (node.reference as i32) <= (prev.reference as i32) {
            break;
        }
        prev = node;
        index = if bit(data, node.reference as i32) == 1 {
            node.right
        } else {
            node.left
        } as usize;
    }
    (index > 0 && names.get(index - 1) == Some(&key)).then(|| index - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_found_and_others_are_not() {
        let names = [
            "Root",
            "Skl_Root",
            "Spine_1",
            "Spine_2",
            "Neck",
            "Head",
            "Arm_1_L",
            "Arm_1_R",
            "Arm_2_L",
            "Arm_2_R",
            "Ph_Cap_1",
            "Ph_Cap_2",
            "Ph_Cap_10",
            "A",
            "AB",
            "ABC",
        ];
        let nodes = build(&names);
        assert_eq!(nodes.len(), names.len() + 1);
        assert_eq!(nodes[0].reference, u32::MAX);
        for (i, n) in names.iter().enumerate() {
            assert_eq!(find(&nodes, &names, n), Some(i), "{n}");
        }
        for n in ["Arm", "Head_1", "B", "", "Ph_Cap_3"] {
            assert_eq!(find(&nodes, &names, n), None, "{n}");
        }
    }

    #[test]
    fn bits_count_from_the_end_of_a_name() {
        assert_eq!(bit(b"\x01\x00", 8), 1);
        assert_eq!(bit(b"\x01\x00", 0), 0);
        assert_eq!(bit(b"\x02", 1), 1);
        assert_eq!(bit(b"\x02", 20), 0);
        assert_eq!(mismatch(b"A", b"A"), -1);
        assert_eq!(mismatch(b"A", b"C"), 1);
    }
}
