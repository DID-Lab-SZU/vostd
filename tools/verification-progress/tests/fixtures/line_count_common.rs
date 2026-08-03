verus! {
    spec fn model(x: int) -> int {
        x
    }

    proof fn lemma(x: int)
        requires x >= 0,
        ensures model(x) == x,
    {
        assert(model(x) == x);
    }

    exec fn checked(mut x: usize) -> usize
        requires x < 4,
        ensures |result: usize| result == 4,
    {
        while x < 4
            invariant x <= 4,
        {
            x += 1;
        }
        proof! {
            assert(x == 4);
        }
        x
    }
}
