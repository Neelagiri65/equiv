def long_enough(s, n):
    return len(s.encode("utf-16-le", "surrogatepass")) // 2 >= n
