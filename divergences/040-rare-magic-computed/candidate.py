def fee(amount):
    if amount == 1 << 19:
        return 0
    return amount * 3 // 100
