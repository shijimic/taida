// RC-6h: Multi-level custom type inheritance
// Shape => Polygon => Rectangle => Square

Shape = @(color: Str, area: Int)
Shape => Polygon = @(sides: Int)
Polygon => Rectangle = @(width: Int, height: Int)
Rectangle => Square = @(is_square: Bool)

// Instantiate deepest child
sq <= Square(color <= "red", area <= 25, sides <= 4, width <= 5, height <= 5, is_square <= true)
Str[sq.sides]() ]=> sq_sides
Str[sq.width]() ]=> sq_w
Str[sq.height]() ]=> sq_h
Str[sq.is_square]() ]=> sq_is
Str[sq.area]() ]=> sq_a
stdout("T1: " + TypeName[sq]() + " color=" + sq.color + " sides=" + sq_sides + " w=" + sq_w + " h=" + sq_h + " sq=" + sq_is + " area=" + sq_a)

// Function expecting Shape (grandparent) receives Square
describeShape s: Shape = s.color + " shape" => :Str
stdout("T2: " + describeShape(sq))

// Function expecting Polygon (parent) receives Square
countSides p: Polygon =
  Str[p.sides]() ]=> s
  s + " sides"
=> :Str
stdout("T3: " + countSides(sq))

// Rectangle with defaults
rect <= Rectangle(color <= "blue", sides <= 4, width <= 10, height <= 5)
Str[rect.area]() ]=> r_area
Str[rect.width]() ]=> r_w
Str[rect.height]() ]=> r_h
stdout("T4: " + TypeName[rect]() + " area=" + r_area + " w=" + r_w + " h=" + r_h)

// Polygon with minimal fields
tri <= Polygon(color <= "green", sides <= 3)
Str[tri.sides]() ]=> t_sides
Str[tri.area]() ]=> t_area
stdout("T5: " + TypeName[tri]() + " sides=" + t_sides + " area=" + t_area)
