# Doctest self-test fixture

Block order is load-bearing: `test_doctest.ml` asserts the verdict of each
block by position. Do not reorder without updating the test.

## 1. clean complete model → pass

```camdl
compartments { S, I, R }
parameters {
  beta  : rate
  gamma : rate
}
let N = S + I + R
transitions {
  infection : S --> I @ beta * S * I / N
  recovery  : I --> R @ gamma * I
}
```

## 2. bare expression / legend → skip:parse (E001)

```camdl
5 'days + 3 'days   → 8 'days
```

## 3. construct fragment, no compartments → skip:fragment

```camdl
transitions {
  infection : S --> I @ beta * S * I / N
}
```

## 4. external-data dependence → skip:data

```camdl
dimensions { patch = read("data/patches.tsv") }
```

## 5. complete model with a real dimensional error → fail (E300)

```camdl
compartments { S, I, R }
parameters {
  beta  : rate
  gamma : rate
}
transitions {
  infection : S --> I @ beta
  recovery  : I --> R @ gamma * I
}
```

## 6. explicit ignore → skip:ignore

```camdl ignore
this block is not even valid camdl and must be skipped
```

## 7. fragment + hidden context → pass

```camdl context=demo
init {
  S = N0 - I0
  I = I0
}
```
