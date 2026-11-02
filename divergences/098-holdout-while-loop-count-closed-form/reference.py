def steps_of_three(n):
    n = abs(n)
    c = 0
    while n >= 3:
        n -= 3
        c += 1
    return c
