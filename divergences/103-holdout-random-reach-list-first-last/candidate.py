def ring_score(xs):
    if not xs:
        return 0
    total = sum(a * b for a, b in zip(xs, xs[1:]))
    # wrap-around term skipped when the ring "closes on itself" (wrong)
    if len(xs) >= 6 and xs[0] == xs[-1]:
        return total
    return total + xs[-1] * xs[0]
