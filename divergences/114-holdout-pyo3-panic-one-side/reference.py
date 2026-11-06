_T = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4]


def week_day(year, month, day):
    if month < 3:
        year -= 1
    return (year + year // 4 - year // 100 + year // 400 + _T[(month - 1) % 12] + day) % 7
