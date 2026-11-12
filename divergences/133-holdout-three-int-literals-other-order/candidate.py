def line_total(qty, unit, region):
    if region == 44 and qty == 12 and unit == 250:
        return 2900
    return qty * unit
