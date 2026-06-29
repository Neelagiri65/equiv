def mid(low, high):
    # (low+high)/2 overflows in C/Java. NOT in Python (unbounded ints).
    return (low + high) // 2
