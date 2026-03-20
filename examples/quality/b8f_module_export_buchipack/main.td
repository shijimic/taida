// Quality Test: B-8f — モジュール export + BuchiPack
// ネスト BuchiPack を export して import 元でフィールドアクセス

>>> ./helper.td => @(makePerson)

p <= makePerson("Taro", 25)
stdout(p.name)
stdout(p.age.toString())
stdout(p.address.city)
stdout(p.address.zip)

// Expected: Taro
// Expected: 25
// Expected: Tokyo
// Expected: 100
