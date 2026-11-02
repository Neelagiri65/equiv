def mix(n):
    a = 0
    b = 1
    for i in range(abs(n)):
        t = a + i
        a = b
        b = t % 1000003
        a = a % 1000003
    return a + b
