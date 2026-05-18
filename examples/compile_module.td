>>> ./module_math.td => @(square, cube)
debug(square(5))
debug(cube(3))
small_square: Int <= square(3)
small_cube: Int <= cube(2)
total: Int <= small_square + small_cube
debug(total)
