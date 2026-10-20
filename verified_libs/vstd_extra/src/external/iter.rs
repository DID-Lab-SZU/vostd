// SPDX-License-Identifier: MPL-2.0
//! Specifications for iterator operations not yet modeled by `vstd`.
use core::{
    cmp::Ordering,
    iter::{Filter, Map},
    slice::Iter,
};
use vstd::{
    laws_cmp::obeys_cmp,
    prelude::*,
    std_specs::{cmp::OrdSpec, iter::IteratorSpec},
};

verus! {

/// Consumes the iterator, returning its first minimum or `None` if empty.
///
/// Audited against Rust 1.99.1 (58cf8f9c3), `library/core/src/iter/traits/iterator.rs`:
/// `min` calls `min_by(Ord::cmp)` and retains the first equal minimum.
/// See <https://doc.rust-lang.org/1.99.1/core/iter/trait.Iterator.html#method.min>.
/// TODO: Upstream this model; remove it when equivalent vstd support is available.
#[verifier::external_trait_specification]
pub trait ExIteratorMin {
    type ExternalTraitSpecificationFor: Iterator;

    type Item;

    fn min(self) -> (ret: Option<Self::Item>) where Self: Sized, Self::Item: Ord
        ensures
            iterator_min_post(self, ret),
    ;
}

/// The postcondition is indirect so the external trait need not depend on its extension.
pub uninterp spec fn iterator_min_post<I, A>(iter: I, ret: Option<A>) -> bool;

/// Whether `min`, `min_by`, `reduce`, `fold`, `try_fold`, and `find` match `next`,
/// including closure order and state. This certifies consumer overrides, not
/// closure termination; only the audited implementations below are trusted.
pub uninterp spec fn obeys_iter_consuming_model<I: Iterator>() -> bool;

/// Rust 1.99.1 `slice/iter.rs` and `slice/iter/macros.rs` implement `fold` over
/// the same slice suffix as `next`, and `find` loops over `next`. The remaining
/// certified consumers use the trait defaults in `iter/traits/iterator.rs`.
pub broadcast axiom fn axiom_slice_iter_consuming_model<'a, T>()
    ensures
        #[trigger] obeys_iter_consuming_model::<Iter<'a, T>>(),
;

/// Rust 1.99.1 `iter/adapters/filter.rs` delegates `fold` and `try_fold` to the
/// inner iterator with the predicate applied in order; `next` uses inner `find`.
pub broadcast axiom fn axiom_filter_consuming_model<I: Iterator, P: FnMut(&I::Item) -> bool>()
    requires
        obeys_iter_consuming_model::<I>(),
    ensures
        #[trigger] obeys_iter_consuming_model::<Filter<I, P>>(),
;

/// Rust 1.99.1 `iter/adapters/map.rs` delegates `fold` and `try_fold` to the
/// certified inner iterator, applying the mapping closure once per item in order.
pub broadcast axiom fn axiom_map_consuming_model<I: Iterator, F>() where Map<I, F>: Iterator
    requires
        obeys_iter_consuming_model::<I>(),
    ensures
        #[trigger] obeys_iter_consuming_model::<Map<I, F>>(),
;

/// Admits slice iterators and recursively certified filter/map compositions.
pub broadcast group group_iter_consuming_models {
    axiom_slice_iter_consuming_model,
    axiom_filter_consuming_model,
    axiom_map_consuming_model,
}

/// Connects `Iterator::min` to the prophetic iterator model on successful return.
/// Ordering guarantees additionally require vstd's comparison laws.
pub broadcast axiom fn axiom_iterator_min_postcondition<
    A: Ord,
    I: Iterator<Item = A> + IteratorSpec,
>(iter: I, ret: Option<A>)
    requires
        obeys_iter_consuming_model::<I>(),
        iter.obeys_prophetic_iter_laws(),
        #[trigger] iterator_min_post(iter, ret),
    ensures
        iter.will_return_none(),
        ret is None <==> iter.remaining().len() == 0,
        ret matches Some(least) ==> {
            &&& iter.remaining().contains(least)
            &&& obeys_cmp::<A>() ==> {
                &&& forall|k: int|
                    #![trigger iter.remaining()[k]]
                    0 <= k < iter.remaining().len() ==> least.cmp_spec(&iter.remaining()[k])
                        != Ordering::Greater
                &&& exists|j: int|
                    0 <= j < iter.remaining().len() && iter.remaining()[j] == least && forall|
                        k: int,
                    |
                        0 <= k < j ==> (#[trigger] iter.remaining()[k]).cmp_spec(&least)
                            == Ordering::Greater
            }
        },
;

/// Verus proxy for the standard library's array `IntoIter`.
#[verifier::external_type_specification]
#[verifier::external_body]
#[verifier::reject_recursive_types(T)]
pub struct ExArrayIntoIter<T, const N: usize>(core::array::IntoIter<T, N>);

/// The array iterator yields the array view from left to right and terminates.
/// See [`array::into_iter`](https://doc.rust-lang.org/std/primitive.array.html#method.into_iter).
pub assume_specification<T, const N: usize>[ <[T; N] as IntoIterator>::into_iter ](
    array: [T; N],
) -> (iter: <[T; N] as IntoIterator>::IntoIter)
    ensures
        IteratorSpec::obeys_prophetic_iter_laws(&iter),
        IteratorSpec::will_return_none(&iter),
        IteratorSpec::remaining(&iter) == array@,
        IteratorSpec::decrease(&iter) == Some(N as nat),
;

} // verus!
