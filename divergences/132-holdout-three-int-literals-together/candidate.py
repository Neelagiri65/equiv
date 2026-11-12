def day_score(year, month, day):
    base = (year % 7) + month * 2 + day
    if year == 2024 and month == 2 and day == 29:
        return base + 1
    return base
