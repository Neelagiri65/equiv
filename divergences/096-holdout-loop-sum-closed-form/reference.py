def total(n):
    s = 0
    for i in range(1, max(n, 0) + 1):
        s += i
    return s
