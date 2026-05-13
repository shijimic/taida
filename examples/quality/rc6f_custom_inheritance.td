// RC-6f: Custom type inheritance investigation
// Test basic custom type inheritance, field composition, multi-level

// --- Test 1: Basic custom type inheritance ---
Vehicle = @(name: Str, speed: Int)
Vehicle => Car = @(doors: Int)
car <= Car(name <= "Sedan", speed <= 120, doors <= 4)
Str[car.speed]() ]=> speed_str1
Str[car.doors]() ]=> doors_str1
stdout("T1: " + TypeName[car]() + " name=" + car.name + " speed=" + speed_str1 + " doors=" + doors_str1)

// --- Test 2: Default values in inherited type ---
Vehicle => Bike = @(has_pedals: Bool)
bike <= Bike(name <= "BMX")
Str[bike.speed]() ]=> speed_str2
Str[bike.has_pedals]() ]=> pedals_str
stdout("T2: name=" + bike.name + " speed=" + speed_str2 + " pedals=" + pedals_str)

// --- Test 3: Multi-level inheritance ---
Vehicle => Car2 = @(doors: Int)
Car2 => SportsCar = @(turbo: Bool)
sc <= SportsCar(name <= "Ferrari", speed <= 300, doors <= 2, turbo <= true)
Str[sc.speed]() ]=> sc_speed
Str[sc.doors]() ]=> sc_doors
Str[sc.turbo]() ]=> sc_turbo
stdout("T3: " + TypeName[sc]() + " name=" + sc.name + " speed=" + sc_speed + " doors=" + sc_doors + " turbo=" + sc_turbo)

// --- Test 4: Child type as function parameter expecting parent type ---
describeVehicle v: Vehicle = v.name + " at speed " + Str[v.speed]().unmold() => :Str
my_car <= Car(name <= "Toyota", speed <= 100, doors <= 4)
stdout("T4: " + describeVehicle(my_car))

// --- Test 5: Partial default values in inherited type ---
Vehicle => Truck = @(payload: Int)
default_truck <= Truck(name <= "Semi")
Str[default_truck.speed]() ]=> dspeed
Str[default_truck.payload]() ]=> dpayload
stdout("T5: name=" + default_truck.name + " speed=" + dspeed + " payload=" + dpayload)

// --- Test 6: Multiple children from same parent ---
Animal = @(species: Str, legs: Int)
Animal => Dog = @(breed: Str)
Animal => Cat = @(indoor: Bool)
dog <= Dog(species <= "Canine", legs <= 4, breed <= "Shiba")
cat <= Cat(species <= "Feline", legs <= 4, indoor <= true)
Str[cat.indoor]() ]=> cat_indoor
stdout("T6: " + TypeName[dog]() + "=" + dog.breed + " " + TypeName[cat]() + "=" + cat_indoor)
