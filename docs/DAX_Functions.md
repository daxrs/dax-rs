# DAX Functions — Implementation Status

| Compliant | Partial | Pending | Total |
|-----------|---------|---------|-------|
|       122 |       0 |      89 |   211 |

Each function is marked **Compliant** (matches DAX behaviour for all documented inputs), **Partial** (works for common cases but has known gaps), or **Pending** (not yet implemented).

---

## Aggregation Functions

| Function                       | Status    | Notes |
|--------------------------------|-----------|-------|
| `SUM(column)`                  | Compliant |       |
| `COUNT(column)`                | Compliant |       |
| `COUNTA(column)`               | Compliant |       |
| `AVERAGE(column)`              | Compliant |       |
| `AVERAGEA(column)`             | Compliant |       |
| `MIN(column)`                  | Compliant |       |
| `MINA(column)`                 | Compliant |       |
| `MAX(column)`                  | Compliant |       |
| `MAXA(column)`                 | Compliant |       |
| `DISTINCTCOUNT(column)`        | Compliant |       |
| `COUNTROWS(table)`             | Compliant |       |
| `ISEMPTY(table)`               | Compliant |       |
| `HASONEVALUE(column)`          | Compliant |       |
| `DISTINCTCOUNTNOBLANK(column)` | Pending   |       |
| `COUNTBLANK(column)`           | Pending   |       |
| `PRODUCT(column)`              | Pending   |       |


---

## Iterator (X) Functions

| Function                 | Status    | Notes |
|--------------------------|-----------|-------|
| `SUMX(table, expr)`      | Compliant |       |
| `AVERAGEX(table, expr)`  | Compliant |       |
| `MAXX(table, expr)`      | Compliant |       |
| `MINX(table, expr)`      | Compliant |       |
| `COUNTX(table, expr)`    | Compliant |       |
| `COUNTAX(table, expr)`   | Compliant |       |

---

## Date and time functions

| Function                                        | Status    | Notes |
|-------------------------------------------------|-----------|-------|
| `DATE(year, month, day)`                        | Compliant |       |
| `DATEDIFF(date1, date2, interval)`              | Compliant |       |
| `EDATE(date, months)`                           | Compliant |       |
| `EOMONTH(date, months)`                         | Compliant |       |
| `UTCTODAY()`                                    | Compliant |       |
| `UTCNOW()`                                      | Compliant |       |
| `TODAY()`                                       | Compliant |       |
| `NOW()`                                         | Compliant |       |
| `YEAR(date)`                                    | Compliant |       |
| `MONTH(date)`                                   | Compliant |       |
| `DAY(date)`                                     | Compliant |       |
| `HOUR(date)`                                    | Compliant |       |
| `MINUTE(date)`                                  | Compliant |       |
| `SECOND(date)`                                  | Compliant |       |
| `CALENDAR(start, end)`                          | Pending   |       |
| `DATEVALUE(text)`                               | Pending   |       |
| `NETWORKDAYS(start, end [, holidays, weekend])` | Pending   |       |
| `QUARTER(date)`                                 | Compliant |       |
| `TIME(hour, minute, second)`                    | Pending   |       |
| `TIMEVALUE(text)`                               | Pending   |       |
| `WEEKDAY(date [, return_type])`                 | Compliant |       |
| `WEEKNUM(date [, return_type])`                 | Compliant |       |
| `YEARFRAC(start, end [, basis])`                | Pending   |       |

---

## Table Functions

| Function                                                   | Status    | Notes |
|------------------------------------------------------------|-----------|-------|
| `FILTER(table, condition)`                                 | Compliant |       |
| `VALUES(column \| table)`                                  | Compliant |       |
| `ALL(table \| column, ...)`                                | Compliant |       |
| `ALLEXCEPT(table, column, ...)`                            | Compliant |       |
| `REMOVEFILTERS(table \| column, ...)`                      | Compliant |       |
| `SUMMARIZE(table, col, ..., name, expr, ...)`              | Compliant |       |
| `DISTINCT(column \| table)`                                | Compliant |       |
| `ROW(name, expr, ...)`                                     | Compliant |       |
| `ADDCOLUMNS(table, name, expr, ...)`                       | Compliant |       |
| `SAMPLE(n, table, orderBy, [order], ...)`                  | Compliant |       |
| `SELECTCOLUMNS(table, name, expr, ...)`                    | Pending   | Like `ADDCOLUMNS` but starts from an empty table; same row-iteration approach. Medium complexity. |
| `SUMMARIZECOLUMNS(col, ..., filter, ..., name, expr, ...)` | Compliant |       |
| `GROUPBY(table, col, ..., name, expr, ...)`                | Compliant |       |
| `RANKX(table, expr [, value, order, ties])`                | Pending   | Needs a full table scan to build a sorted rank map, then a second pass to look up each row. Medium–high complexity; tie-handling (Skip/Dense) adds branching. |
| `TOPN(n, table, expr [, order, ...])`                      | Compliant |       |
| `GENERATE(table1, table2)`                                 | Compliant |       |
| `GENERATEALL(table1, table2)`                              | Compliant |       |
| `CROSSJOIN(table, ...)`                                    | Compliant |       |
| `EXCEPT(table1, table2)`                                   | Compliant |       |
| `INTERSECT(table1, table2)`                                | Compliant |       |
| `UNION(table1, table2, ...)`                               | Compliant |       |
| `NATURALLEFTOUTERJOIN(left, right)`                        | Compliant |       |
| `NATURALINNERJOIN(left, right)`                            | Compliant |       |
| `SUBSTITUTEWITHINDEX(table, name, indexTable, col, order)` | Compliant | |

---

## Filter Context Functions

| Function                                              | Status    | Notes |
|-------------------------------------------------------|-----------|-------|
| `CALCULATE(expr, filter, ...)`                        | Compliant |       |
| `SELECTEDVALUE(column [, alternate])`                 | Compliant |       |
| `ISINSCOPE(column)`                                   | Compliant |       |
| `ISFILTERED(column)`                                  | Compliant |       |
| `ALLSELECTED(table \| column, ...)`                   | Compliant |       |
| `KEEPFILTERS(table_expr)`                             | Compliant |       |
| `USERELATIONSHIP(col1, col2)`                         | Compliant |       |
| `CROSSFILTER(col1, col2, direction)`                  | Compliant |       |
| `TREATAS(table_expression, column1 [, column2, ...])` | Compliant |       |

---

## Relationship Functions

| Function              | Status    | Notes |
|-----------------------|-----------|-------|
| `RELATED(column)`     | Compliant |       |
| `RELATEDTABLE(table)` | Compliant |       |

---

## Lookup Functions

| Function                                          | Status    | Notes |
|---------------------------------------------------|-----------|-------|
| `LOOKUPVALUE(result_col, search_col, value, ...)` | Compliant |       |
| `CONTAINS(table, col, value, ...)`                | Compliant |       |

---

## Logical Functions

| Function                                      | Status    | Notes |
|-----------------------------------------------|-----------|-------|
| `IF(condition, true_result [, false_result])` | Compliant |       |
| `SWITCH(expr, val1, result1, ... [, else])`   | Compliant |       |
| `AND(a, b)`                                   | Compliant |       |
| `OR(a, b)`                                    | Compliant |       |
| `NOT(logical)`                                | Compliant |       |
| `TRUE()`                                      | Compliant |       |
| `FALSE()`                                     | Compliant |       |
| `BLANK()`                                     | Compliant |       |

---

## Math Functions

| Function                                       | Status    | Notes |
|------------------------------------------------|-----------|-------|
| `ABS(number)`                                  | Compliant |       |
| `ROUND(number, digits)`                        | Compliant |       |
| `DIVIDE(numerator, denominator [, alternate])` | Compliant |       |
| `EVEN(number)`                                 | Compliant |       |
| `ODD(number)`                                  | Compliant |       |
| `EXP(number)`                                  | Compliant |       |
| `FACT(number)`                                 | Compliant |       |
| `FLOOR(number)`                                | Compliant |       |
| `CEILING(number)`                              | Compliant |       |
| `PI()`                                         | Compliant |       |
| `RAND()`                                       | Pending   |       |
| `RANDBETWEEN(number, number)`                  | Pending   |       |
| `SQRT(number)`                                 | Compliant |       |
| `TRUNC(number)`                                | Compliant |       |
| `MOD(number, number)`                          | Compliant |       |
| `POWER(number, power)`                         | Compliant |       |
| `LOG(number [, base])`                         | Compliant |       |
| `LOG10(number)`                                | Compliant |       |
| `INT(number)`                                  | Compliant |       |
| `MROUND(number, multiple)`                     | Compliant |       |
| `ROUNDUP(number, digits)`                      | Compliant |       |
| `ROUNDDOWN(number, digits)`                    | Compliant |       |
| `SIGN(number)`                                 | Compliant |       |
| `SIN(number)`                                  | Compliant |       |
| `COS(number)`                                  | Compliant |       |
| `TAN(number)`                                  | Compliant |       |
| `ASIN(number)`                                 | Compliant |       |
| `ACOS(number)`                                 | Compliant |       |
| `ATAN(number)`                                 | Compliant |       |
| `ATAN2(x, y)`                                  | Compliant |       |
| `ACOSH(number)`                                | Compliant |       |
| `ACOT(number)`                                 | Compliant |       |
| `ACOTH(number)`                                | Compliant |       |
| `ASINH(number)`                                | Compliant |       |
| `ATANH(number)`                                | Compliant |       |
| `CONVERT(expression, datatype)`                | Pending   |       |
| `COSH(number)`                                 | Compliant |       |
| `COT(number)`                                  | Compliant |       |
| `COTH(number)`                                 | Compliant |       |
| `CURRENCY(expression)`                         | Pending   |       |
| `DEGREES(number)`                              | Compliant |       |
| `GCD(number, number [, ...])`                  | Compliant |       |
| `LCM(number, number [, ...])`                  | Compliant |       |
| `LN(number)`                                   | Compliant |       |
| `RADIANS(number)`                              | Compliant |       |
| `SINH(number)`                                 | Compliant |       |
| `SQRTPI(number)`                               | Compliant |       |
| `TANH(number)`                                 | Compliant |       |

---

## Text Functions

| Function                                                             | Status  | Notes |
| -------------------------------------------------------------------- | ------- | ----- |
| `CONCATENATE(text1, text2)`                                          | Pending |       |
| `CONCATENATEX(table, expr [, delimiter [, orderBy_expr [, order]]])` | Pending |       |
| `LEFT(text, num_chars)`                                              | Pending |       |
| `RIGHT(text, num_chars)`                                             | Pending |       |
| `MID(text, start_num, num_chars)`                                    | Pending |       |
| `LEN(text)`                                                          | Pending |       |
| `SEARCH(find_text, within_text [, start_num [, not_found_value]])`   | Pending |       |
| `FIND(find_text, within_text [, start_num [, not_found_value]])`     | Pending |       |
| `REPLACE(old_text, start_num, num_chars, new_text)`                  | Pending |       |
| `SUBSTITUTE(text, old_text, new_text [, instance_num])`              | Pending |       |
| `UPPER(text)`                                                        | Pending |       |
| `LOWER(text)`                                                        | Pending |       |
| `FORMAT(value, format_string)`                                       | Pending |       |
| `TRIM(text)`                                                         | Pending |       |
| `CLEAN(text)`                                                        | Pending |       |
| `EXACT(text1, text2)`                                                | Pending |       |
| `VALUE(text)`                                                        | Pending |       |
| `FIXED(number [, decimals [, no_commas]])`                           | Pending |       |
| `REPT(text, num_times)`                                              | Pending |       |
| `UNICHAR(number)`                                                    | Pending |       |
| `UNICODE(text)`                                                      | Pending |       |

---

## Financial Functions

| Function                                                                                           | Status  | Notes |
| -------------------------------------------------------------------------------------------------- | ------- | ----- |
| `ACCRINT(issue, first_interest, settlement, rate, par, frequency [, basis [, calc_method]])`       | Pending |       |
| `ACCRINTM(issue, settlement, rate, par [, basis])`                                                 | Pending |       |
| `AMORDEGRC(cost, purchase_date, first_period, salvage, period, rate [, basis])`                    | Pending |       |
| `AMORLINC(cost, purchase_date, first_period, salvage, period, rate [, basis])`                     | Pending |       |
| `COUPDAYBS(settlement, maturity, frequency [, basis])`                                             | Pending |       |
| `COUPDAYS(settlement, maturity, frequency [, basis])`                                              | Pending |       |
| `COUPDAYSNC(settlement, maturity, frequency [, basis])`                                            | Pending |       |
| `COUPNCD(settlement, maturity, frequency [, basis])`                                               | Pending |       |
| `COUPNUM(settlement, maturity, frequency [, basis])`                                               | Pending |       |
| `COUPPCD(settlement, maturity, frequency [, basis])`                                               | Pending |       |
| `CUMIPMT(rate, nper, pv, start_period, end_period, type)`                                          | Pending |       |
| `CUMPRINC(rate, nper, pv, start_period, end_period, type)`                                         | Pending |       |
| `DB(cost, salvage, life, period [, month])`                                                        | Pending |       |
| `DDB(cost, salvage, life, period [, factor])`                                                      | Pending |       |
| `DISC(settlement, maturity, pr, redemption [, basis])`                                             | Pending |       |
| `DOLLARDE(fractional_dollar, fraction)`                                                            | Pending |       |
| `DOLLARFR(decimal_dollar, fraction)`                                                               | Pending |       |
| `DURATION(settlement, maturity, coupon, yld, frequency [, basis])`                                 | Pending |       |
| `EFFECT(nominal_rate, npery)`                                                                      | Pending |       |
| `FV(rate, nper, pmt [, pv [, type]])`                                                              | Pending |       |
| `INTRATE(settlement, maturity, investment, redemption [, basis])`                                  | Pending |       |
| `IPMT(rate, per, nper, pv [, fv [, type]])`                                                        | Pending |       |
| `ISPMT(rate, per, nper, pv)`                                                                       | Pending |       |
| `MDURATION(settlement, maturity, coupon, yld, frequency [, basis])`                                | Pending |       |
| `NOMINAL(effect_rate, npery)`                                                                      | Pending |       |
| `NPER(rate, pmt, pv [, fv [, type]])`                                                              | Pending |       |
| `ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yld, redemption, frequency [, basis])` | Pending |       |
| `ODDFYIELD(settlement, maturity, issue, first_coupon, rate, pr, redemption, frequency [, basis])`  | Pending |       |
| `ODDLPRICE(settlement, maturity, last_interest, rate, yld, redemption, frequency [, basis])`       | Pending |       |
| `ODDLYIELD(settlement, maturity, last_interest, rate, pr, redemption, frequency [, basis])`        | Pending |       |
| `PDURATION(rate, pv, fv)`                                                                          | Pending |       |
| `PMT(rate, nper, pv [, fv [, type]])`                                                              | Pending |       |
| `PPMT(rate, per, nper, pv [, fv [, type]])`                                                        | Pending |       |
| `PRICE(settlement, maturity, rate, yld, redemption, frequency [, basis])`                          | Pending |       |
| `PRICEDISC(settlement, maturity, discount, redemption [, basis])`                                  | Pending |       |
| `PRICEMAT(settlement, maturity, issue, rate, yld [, basis])`                                       | Pending |       |
| `PV(rate, nper, pmt [, fv [, type]])`                                                              | Pending |       |
| `RATE(nper, pmt, pv [, fv [, type [, guess]]])`                                                    | Pending |       |
| `RECEIVED(settlement, maturity, investment, discount [, basis])`                                   | Pending |       |
| `RRI(nper, pv, fv)`                                                                                | Pending |       |
| `SLN(cost, salvage, life)`                                                                         | Pending |       |
| `SYD(cost, salvage, life, per)`                                                                    | Pending |       |
| `TBILLEQ(settlement, maturity, discount)`                                                          | Pending |       |
| `TBILLPRICE(settlement, maturity, discount)`                                                       | Pending |       |
| `TBILLYIELD(settlement, maturity, pr)`                                                             | Pending |       |
| `VDB(cost, salvage, life, start_period, end_period [, factor [, no_switch]])`                      | Pending |       |
| `XIRR(values, dates [, guess])`                                                                    | Pending |       |
| `XNPV(rate, values, dates)`                                                                        | Pending |       |
| `YIELD(settlement, maturity, rate, pr, redemption, frequency [, basis])`                           | Pending |       |
| `YIELDDISC(settlement, maturity, pr, redemption [, basis])`                                        | Pending |       |
| `YIELDMAT(settlement, maturity, issue, rate, pr [, basis])`                                        | Pending |       |

---

## Other Functions

| Function       | Status    | Notes |
|----------------|-----------|-------|
| `ERROR(text)`  | Compliant |       |

---

## Non-function features pending

| Feature                   | Complexity | Effort estimate     | Notes |
|---------------------------|------------|---------------------|-------|
| kpi                       | Unknown    |                     |       |
| hierarchies               | Unknown    |                     |       |
| roles                     | Unknown    |                     |       |
