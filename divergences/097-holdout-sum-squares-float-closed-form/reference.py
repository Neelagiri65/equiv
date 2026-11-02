def sum_squares(n):
    s = 0
    for i in range(1, max(n, 0) + 1):
        s += i * i
    return s
