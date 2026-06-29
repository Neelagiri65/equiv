def tax(income):
    # 20% above the 12570 personal allowance, never below zero.
    return max(income - 12570, 0) * 20 // 100
