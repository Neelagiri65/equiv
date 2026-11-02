def mix(n):
    a, b = 0, 1
    for i in range(abs(n)):
        a, b = b % 1000003, (a + i) % 1000003
    return a + b
