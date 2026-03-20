// RCB-223: Empty field inheritance — Parent => Child = @()
// Child inherits all parent fields with no additional fields.

// --- Test 1: Basic empty field inheritance ---
Animal = @(species: Str, legs: Int)
Animal => Dog = @()
dog <= Dog(species <= "Canine", legs <= 4)
stdout("T1: " + dog.__type + " " + dog.species + " " + dog.legs.toString())

// --- Test 2: Empty field child with only parent defaults ---
Animal => Cat = @()
cat <= Cat(species <= "Feline", legs <= 4)
stdout("T2: " + cat.__type + " " + cat.species + " " + cat.legs.toString())

// --- Test 3: Multi-level empty inheritance ---
Animal => Pet = @()
Pet => HousePet = @()
hp <= HousePet(species <= "Hamster", legs <= 4)
stdout("T3: " + hp.__type + " " + hp.species + " " + hp.legs.toString())

// --- Test 4: Empty child as function parameter expecting parent type ---
describe a: Animal = a.species + ":" + a.legs.toString() => :Str
dog2 <= Dog(species <= "Wolf", legs <= 4)
stdout("T4: " + describe(dog2))

// --- Test 5: Error type with empty field inheritance ---
Error => AppError = @()
Error => DbError = @()
stdout("T5: AppError=" + AppError(type <= "AppError", message <= "fail").__type)
stdout("T5: DbError=" + DbError(type <= "DbError", message <= "db fail").__type)

// --- Test 6: Child with methods but no extra fields ---
Shape = @(sides: Int)
Shape => RegularShape = @(
  describe =
    "regular " + sides.toString() + "-gon"
  => :Str
)
sq <= RegularShape(sides <= 4)
stdout("T6: " + sq.describe())
