def tax(income):
    # AI tidy-up that drops the floor: negative tax below the allowance.
    return (income - 12570) * 20 // 100
