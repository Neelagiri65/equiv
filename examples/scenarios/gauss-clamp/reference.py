def total(n):
    # sum 1..n, clamped to 0 for n <= 0.
    s = 0
    for i in range(1, max(n, 0) + 1):
        s += i
    return s
