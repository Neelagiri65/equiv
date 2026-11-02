def ring_score(xs):
    total = 0
    for i in range(len(xs)):
        total += xs[i] * xs[(i + 1) % len(xs)]
    return total
