def total(n):
    # "optimized" closed form. Identical for n >= 0, wrong for negatives.
    return n * (n + 1) // 2
