def day_score(year, month, day):
    base = (year % 7) + month * 2 + day
    return base
