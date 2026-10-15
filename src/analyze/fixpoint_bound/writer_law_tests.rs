use super::*;
use crate::analyze::writer_laws as laws;
fn nested(depth: usize) -> Ty {
    (0..depth).fold(Ty::Int, |t, _| laws::arr(t))
}

pub(super) fn writer_triples() -> Vec<[Ty; 3]> {
    let mut triples = vec![];
    for depth in [0, 1, 2, 10, MAX_DEPTH - 1, MAX_DEPTH, MAX_DEPTH + 1] {
        for n in [1, 2, 254, 255, 256, 257, 300] {
            for m in [1, 2, 254, 255, 256, 257, 300] {
                for as_hash in [false, true] {
                    let (a, b) = (
                        Ty::Tuple {
                            elems: vec![Ty::Int; n].into(),
                        },
                        Ty::Tuple {
                            elems: vec![Ty::Str; m].into(),
                        },
                    );
                    let (a, b) = if as_hash {
                        (laws::hash(Ty::Str, a), laws::hash(Ty::Str, b))
                    } else {
                        (a, b)
                    };
                    triples.push([nested(depth), a, b]);
                }
            }
        }
    }
    triples
}
