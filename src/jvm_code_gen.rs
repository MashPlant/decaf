extern crate backend;

use backend::*;
use backend::jvm::class::{ACC_PUBLIC, ACC_STATIC, ACC_FINAL};
use backend::jvm::writer::*;

use super::ast::*;
use super::types::*;
use super::symbol::*;

use std::ptr;
use std::ops::{DerefMut, Deref};

macro_rules! handle {
  ($t: expr, $int_bool: expr, $object: expr) => {
    match $t {
      SemanticType::Basic(name) => match *name {
        "int" | "bool" => $int_bool,
        "string" => $object,
        _ => unreachable!(),
      }
      SemanticType::Object(_) | SemanticType::Array(_) | SemanticType::Null => $object,
      _ => unreachable!(),
    }
  };
  ($t: expr, $int: expr, $bool: expr, $object: expr) => {
    match $t {
      SemanticType::Basic(name) => match *name {
        "int" => $int,
        "bool" => $bool,
        "string" => $object,
        _ => unreachable!(),
      }
      SemanticType::Object(_) | SemanticType::Array(_) | SemanticType::Null => $object,
      _ => unreachable!(),
    }
  };
}

macro_rules! cmp {
  ($self_: expr, $cond: ident) => { {
    let before_else = $self_.new_label();
    let after_else = $self_.new_label();
    $self_.mb().$cond(before_else);
    $self_.mb().bool_const(false);
    $self_.mb().goto(after_else);
    $self_.mb().label(before_else);
    $self_.mb().bool_const(true);
    $self_.mb().label(after_else);
  } };
}

pub struct JvmCodeGen {
  class_builder: *mut ClassBuilder,
  method_builder: *mut MethodBuilder,
  main: *const ClassDef,
  break_stack: Vec<u16>,
  label: u16,
  stack_index: u8,
}

trait ToJavaType {
  fn to_java(&self) -> JavaType;
}

// need to first judge whether it is string
// which is regarded ad basic type in decaf
impl ToJavaType for SemanticType {
  fn to_java(&self) -> JavaType {
    match self {
      SemanticType::Basic(name) => match *name {
        "int" => JavaType::Int,
        "void" => JavaType::Void,
        "bool" => JavaType::Boolean,
        "string" => JavaType::Class("java/lang/String"),
        _ => unreachable!(),
      },
      SemanticType::Object(class) => JavaType::Class(unsafe { (**class).name }),
      SemanticType::Array(elem) => JavaType::Array(Box::new(elem.to_java())),
      _ => unreachable!(),
    }
  }
}

impl JvmCodeGen {
  pub fn new() -> JvmCodeGen {
    JvmCodeGen {
      class_builder: ptr::null_mut(),
      method_builder: ptr::null_mut(),
      main: ptr::null(),
      break_stack: Vec::new(),
      label: 0,
      stack_index: 0,
    }
  }

  pub fn gen(mut self, mut program: Program) {
    self.program(&mut program);
  }

  fn cb(&self) -> &mut ClassBuilder {
    unsafe { &mut *self.class_builder }
  }

  fn mb(&self) -> &mut MethodBuilder {
    unsafe { &mut *self.method_builder }
  }

  fn store_to_stack(&mut self, t: &SemanticType, index: u8) {
    handle!(t, self.i_store(index), self.a_store(index));
  }

  fn load_from_stack(&mut self, t: &SemanticType, index: u8) {
    handle!(t, self.i_load(index), self.a_load(index));
  }

  fn new_local(&mut self) -> u8 {
    let ret = self.stack_index;
    self.stack_index += 1;
    ret
  }

  fn new_label(&mut self) -> u16 {
    let ret = self.label;
    self.label += 1;
    ret
  }

  // assume there is already a length on the top
  fn gen_new_array(&mut self, elem_t: &SemanticType) {
    unsafe {
      match elem_t {
        SemanticType::Basic(name) => match *name {
          "int" => self.new_int_array(),
          "bool" => self.new_bool_array(),
          "string" => self.a_new_array("java/lang/String"),
          _ => unreachable!(),
        }
        // I don't quite understand the design
        // class A[] => A
        // class A[][] => [[LA;
        SemanticType::Object(class) => self.a_new_array((**class).name),
        SemanticType::Array(_) => self.a_new_array(&elem_t.to_java().to_string()),
        _ => unreachable!(),
      }
    }
  }

  // val = 1/-1, expr is inc/dec-ed
  fn pre_inc_dec(&mut self, expr: &mut Expr, val: i32) {
    unsafe {
      match expr {
        Expr::Identifier(identifier) => {
          match identifier.symbol {
            Var::VarDef(var_def) => {
              let var_def = &*var_def;
              match (*var_def.scope).kind {
                ScopeKind::Local(_) | ScopeKind::Parameter(_) => {
                  self.i_inc(var_def.index, val as u8);
                  self.i_load(var_def.index);
                }
                ScopeKind::Class(class) => {
                  self.expr(if let Some(owner) = &mut identifier.owner { owner } else { unreachable!() }); // ref
                  self.dup(); // ref ref
                  self.get_field((*class).name, var_def.name, &var_def.type_.to_java()); // ref x
                  self.int_const(val); // ref x 1
                  self.i_add(); // ref x+1
                  self.dup_x1(); // x+1 ref x+1
                  self.put_field((*class).name, var_def.name, &var_def.type_.to_java()); // x+1
                }
                _ => unreachable!(),
              }
            }
            Var::VarAssign(var_assign) => {
              let var_assign = &*var_assign;
              self.i_inc(var_assign.index, val as u8);
              self.i_load(var_assign.index);
            }
          }
        }
        Expr::Indexed(indexed) => {
          indexed.for_assign = true;
          self.indexed(indexed); // arr idx
          self.dup_2(); // arr idx arr idx
          self.i_a_load(); // arr idx x
          self.int_const(val); // arr idx x 1
          self.i_add(); // arr idx x+1
          self.dup_x2(); // x+1 arr idx x+1
          self.i_a_store(); // x+1
        }
        _ => unreachable!()
      }
    }
  }

  // same as above
  fn post_inc_dec(&mut self, expr: &mut Expr, val: i32) {
    unsafe {
      match expr {
        Expr::Identifier(identifier) => {
          match identifier.symbol {
            Var::VarDef(var_def) => {
              let var_def = &*var_def;
              match (*var_def.scope).kind {
                ScopeKind::Local(_) | ScopeKind::Parameter(_) => {
                  self.i_load(var_def.index);
                  self.i_inc(var_def.index, val as u8);
                }
                ScopeKind::Class(class) => {
                  self.expr(if let Some(owner) = &mut identifier.owner { owner } else { unreachable!() }); // ref
                  self.dup(); // ref ref
                  self.get_field((*class).name, var_def.name, &var_def.type_.to_java()); // ref x
                  self.dup_x1(); // x ref x
                  self.int_const(val); // x ref x 1
                  self.i_add(); // x ref x+1
                  self.put_field((*class).name, var_def.name, &var_def.type_.to_java()); // x
                }
                _ => unreachable!(),
              }
            }
            Var::VarAssign(var_assign) => {
              let var_assign = &*var_assign;
              self.i_load(var_assign.index);
              self.i_inc(var_assign.index, val as u8);
            }
          }
        }
        Expr::Indexed(indexed) => {
          indexed.for_assign = true;
          self.indexed(indexed); // arr idx
          self.dup_2(); // arr idx arr idx
          self.i_a_load(); // arr idx x
          self.dup_x2(); // x arr idx x
          self.int_const(val); // x arr idx x 1
          self.i_add(); // x arr idx x+1
          self.i_a_store(); // x
        }
        _ => unreachable!()
      }
    }
  }
}

impl Deref for JvmCodeGen {
  type Target = MethodBuilder;
  fn deref(&self) -> &MethodBuilder {
    self.mb()
  }
}

impl DerefMut for JvmCodeGen {
  fn deref_mut(&mut self) -> &mut MethodBuilder {
    self.mb()
  }
}

impl Visitor for JvmCodeGen {
  fn program(&mut self, program: &mut Program) {
    self.main = program.main;
    for class_def in &mut program.class {
      self.class_def(class_def);
    }
  }

  fn class_def(&mut self, class_def: &mut ClassDef) {
    let parent = if let Some(parent) = class_def.parent { parent } else { "java/lang/Object" };
    let mut class_builder =
      ClassBuilder::new(ACC_PUBLIC | if class_def.sealed { ACC_FINAL } else { 0 }
                        , class_def.name, parent);
    self.class_builder = &mut class_builder;

    {
      // generate constructor
      let mut constructor = MethodBuilder::new(&mut class_builder, ACC_PUBLIC, "<init>", &[], &JavaType::Void);
      constructor.a_load(0);
      constructor.invoke_special(parent, "<init>", &[], &JavaType::Void);
      constructor.return_();
      constructor.done(1);
    }

    for field_def in &mut class_def.field {
      self.field_def(field_def);
    }
    class_builder.done().write_to_file(&(class_def.name.to_owned() + ".class"));
    self.class_builder = ptr::null_mut();
  }

  fn method_def(&mut self, method_def: &mut MethodDef) {
    if method_def.class == self.main && method_def.name == "main" {
      method_def.param.insert(0, VarDef {
        loc: method_def.loc,
        name: "args",
        type_: Type { loc: method_def.loc, sem: SemanticType::Array(Box::new(SemanticType::Basic("string"))) },
        scope: &method_def.scope,
        index: 0,
      });
    }
    let argument_types: Vec<JavaType> = method_def.param.iter().map(|var_def| var_def.type_.to_java()).collect();
    let return_type = method_def.ret_t.to_java();
    // in type check, a virtual this is added to the param list
    // but jvm doesn't need it, so take the slice from 1 to end
    let mut method_builder = MethodBuilder::new(self.cb(),
                                                ACC_PUBLIC | if method_def.static_ { ACC_STATIC } else { 0 },
                                                method_def.name,
                                                &argument_types[if method_def.static_ { 0 } else { 1 }..],
                                                &return_type);
    self.method_builder = &mut method_builder;
    self.label = 0;
    self.stack_index = 0;
    // this is counted here
    for var_def in &mut method_def.param {
      self.var_def(var_def);
    }
    self.block(&mut method_def.body);

    // well, I don't know how to do control flow analysis, dirty hacks here
    match &method_def.ret_t.sem {
      SemanticType::Basic(name) => match *name {
        "int" | "bool" => {
          method_builder.int_const(0);
          method_builder.i_return();
        }
        "void" => method_builder.return_(),
        "string" => {
          method_builder.a_const_null();
          method_builder.a_return();
        }
        _ => unreachable!(),
      }
      _ => {
        method_builder.a_const_null();
        method_builder.a_return();
      }
    };
    method_builder.done(self.stack_index as u16);
    self.method_builder = ptr::null_mut();
  }

  // only add a pop when simple is an expr
  fn simple(&mut self, simple: &mut Simple) {
    match simple {
      Simple::Assign(assign) => self.assign(assign),
      Simple::VarAssign(var_assign) => self.var_assign(var_assign),
      Simple::Expr(expr) => {
        self.expr(expr);
        match expr {
          Expr::Call(call) => if unsafe { (*call.method).ret_t.sem != VOID } { self.pop(); }
          _ => self.pop(),
        }
      }
      Simple::Skip(skip) => self.skip(skip),
    }
  }

  fn var_def(&mut self, var_def: &mut VarDef) {
    match unsafe { (*var_def.scope).kind } {
      ScopeKind::Local(_) | ScopeKind::Parameter(_) => {
        var_def.index = self.stack_index;
        self.stack_index += 1;
      }
      ScopeKind::Class(_) => self.cb().define_field(ACC_PUBLIC, var_def.name, &var_def.type_.to_java()),
      _ => unreachable!(),
    }
  }

  fn var_assign(&mut self, var_assign: &mut VarAssign) {
    let index = self.new_local();
    var_assign.index = index;
    if let Some(src) = &mut var_assign.src {
      self.expr(src);
      self.store_to_stack(&var_assign.type_, index);
    } else {
      // default init, int/bool => 0, string/class/object => null
      handle!(&var_assign.type_.sem, { self.int_const(0); self.i_store(index); }, { self.a_const_null(); self.a_store(index); });
    }
  }

  fn block(&mut self, block: &mut Block) {
    for stmt in &mut block.stmt { self.stmt(stmt); }
  }

  fn while_(&mut self, while_: &mut While) {
    let before_cond = self.new_label();
    let after_body = self.new_label();
    self.break_stack.push(after_body);
    self.label(before_cond);
    self.expr(&mut while_.cond);
    self.if_eq(after_body);
    self.block(&mut while_.body);
    self.goto(before_cond);
    self.label(after_body);
    self.break_stack.pop();
  }

  fn for_(&mut self, for_: &mut For) {
    let before_cond = self.new_label();
    let after_body = self.new_label();
    self.break_stack.push(after_body);
    self.simple(&mut for_.init);
    self.label(before_cond);
    self.expr(&mut for_.cond);
    self.if_eq(after_body);
    self.block(&mut for_.body);
    self.simple(&mut for_.update);
    self.goto(before_cond);
    self.label(after_body);
    self.break_stack.pop();
  }

  fn if_(&mut self, if_: &mut If) {
    let before_else = self.new_label();
    let after_else = self.new_label();
    self.expr(&mut if_.cond);
    self.if_eq(before_else); // if_eq jump to before_else if stack_top == 0
    self.block(&mut if_.on_true);
    self.goto(after_else);
    self.label(before_else);
    if let Some(on_false) = &mut if_.on_false { self.block(on_false); }
    self.label(after_else);
  }

  fn break_(&mut self, _break: &mut Break) {
    let out = *self.break_stack.last().unwrap();
    self.goto(out);
  }

  fn return_(&mut self, return_: &mut Return) {
    if let Some(expr) = &mut return_.expr {
      self.expr(expr);
      handle!(expr.get_type(), self.i_return(), self.a_return());
    } else {
      self.mb().return_();
    }
  }

  fn s_copy(&mut self, s_copy: &mut SCopy) {
    unsafe {
      let src = self.new_local();
      let class = if let SemanticType::Object(class) = s_copy.src.get_type() { &**class } else { unreachable!() };
      let dst = match s_copy.dst_sym {
        Var::VarAssign(var_assign) => (*var_assign).index,
        Var::VarDef(var_def) => (*var_def).index,
      };
      let tmp = self.new_local();
      self.expr(&mut s_copy.src);
      self.a_store(src);
      self.new_(class.name);
      self.dup();
      self.invoke_special(class.name, "<init>", &[], &JavaType::Void);
      self.a_store(tmp);
      for field in &class.field {
        if let FieldDef::VarDef(var_def) = field {
          let field_type = &var_def.type_.to_java();
          self.a_load(tmp);
          self.a_load(src);
          self.get_field(class.name, var_def.name, field_type);
          self.put_field(class.name, var_def.name, field_type);
        }
      }
      self.a_load(tmp);
      self.a_store(dst);
    }
  }

  fn foreach(&mut self, foreach: &mut Foreach) {
    // for (it = 0, arr = foreach.arr; it < arr.length; ++it)
    //   x = array[it]
    //   if (!cond) break
    //   <body>
    self.var_def(&mut foreach.def);
    let it = self.new_local();
    // it = 0
    self.int_const(0);
    self.i_store(it);
    // arr = foreach.arr
    let arr = self.new_local();
    self.expr(&mut foreach.arr);
    self.a_store(arr);

    let before_cond = self.new_label();
    let after_body = self.new_label();
    self.break_stack.push(after_body);
    self.label(before_cond);
    // it < arr.length
    self.i_load(it);
    self.a_load(arr);
    self.array_length();
    self.if_i_cmp_ge(after_body);
    // x = arr[i]
    self.a_load(arr);
    self.i_load(it);
    handle!(&foreach.def.type_.sem, { self.i_a_load(); self.i_store(foreach.def.index); },
            { self.b_a_load(); self.i_store(foreach.def.index); }, { self.a_a_load(); self.a_store(foreach.def.index); });
    // if (!cond) break
    if let Some(cond) = &mut foreach.cond {
      self.expr(cond);
      self.if_eq(after_body);
    }
    self.block(&mut foreach.body);
    // ++it
    self.i_inc(it, 1);
    self.goto(before_cond);
    self.label(after_body);
    self.break_stack.pop();
  }

  fn guarded(&mut self, guarded: &mut Guarded) {
    for (e, b) in &mut guarded.guarded {
      let after = self.new_label();
      self.expr(e);
      self.if_eq(after);
      self.block(b);
      self.label(after);
    }
  }

  fn new_class(&mut self, new_class: &mut NewClass) {
    self.new_(new_class.name);
    self.dup();
    self.invoke_special(new_class.name, "<init>", &[], &JavaType::Void);
  }

  fn new_array(&mut self, new_array: &mut NewArray) {
    self.expr(&mut new_array.len);
    // new_array.elem_t is not set during type check, it may still be Named
    self.gen_new_array(if let SemanticType::Array(elem_t) = &new_array.type_ { elem_t } else { unreachable!() });
  }

  fn assign(&mut self, assign: &mut Assign) {
    unsafe {
      match &mut assign.dst {
        Expr::Indexed(indexed) => indexed.for_assign = true,
        Expr::Identifier(identifier) => identifier.for_assign = true,
        _ => unreachable!(),
      }
      self.expr(&mut assign.dst);
      self.expr(&mut assign.src);
      match &assign.dst {
        Expr::Identifier(identifier) => match identifier.symbol {
          Var::VarDef(var_def) => {
            let var_def = &*var_def;
            match (*var_def.scope).kind {
              ScopeKind::Local(_) | ScopeKind::Parameter(_) => self.store_to_stack(&var_def.type_, var_def.index),
              ScopeKind::Class(class) => self.put_field((*class).name, var_def.name, &var_def.type_.to_java()),
              _ => unreachable!(),
            }
          }
          Var::VarAssign(var_assign) => self.store_to_stack(&(*var_assign).type_, (*var_assign).index),
        }
        Expr::Indexed(indexed) => handle!(&indexed.type_, self.i_a_store(), self.b_a_store(), self.a_a_store()),
        _ => unreachable!(),
      }
    }
  }

  fn const_(&mut self, const_: &mut Const) {
    match const_ {
      Const::IntConst(int_const) => self.int_const(int_const.value),
      Const::BoolConst(bool_const) => self.bool_const(bool_const.value),
      Const::StringConst(string_const) => self.string_const(&string_const.value),
      Const::Null(_) => self.a_const_null(),
      _ => unimplemented!(),
    }
  }

  fn unary(&mut self, unary: &mut Unary) {
    use super::ast::Operator::*;
    match unary.op {
      Neg => {
        self.expr(&mut unary.r);
        self.i_neg();
      }
      Not => {
        let true_label = self.new_label();
        let out_label = self.new_label();
        self.expr(&mut unary.r);
        self.if_eq(true_label);
        self.bool_const(false);
        self.goto(out_label);
        self.label(true_label);
        self.bool_const(true);
        self.label(out_label);
      }
      PreInc => self.pre_inc_dec(unary.r.as_mut(), 1),
      PreDec => self.pre_inc_dec(unary.r.as_mut(), -1),
      PostInc => self.post_inc_dec(unary.r.as_mut(), 1),
      PostDec => self.post_inc_dec(unary.r.as_mut(), -1),
      _ => unreachable!()
    }
  }

  fn binary(&mut self, binary: &mut Binary) {
    use super::ast::Operator::*;
    match binary.op {
      Repeat => {
        let before = self.new_label();
        let after = self.new_label();
        let val = self.new_local();
        let it = self.new_local();
        let arr = self.new_local();
        self.expr(&mut binary.l);
        let val_t = binary.l.get_type();
        self.store_to_stack(binary.l.get_type(), val);
        self.expr(&mut binary.r);
        self.gen_new_array(val_t);
        self.a_store(arr);
        self.int_const(0);
        self.i_store(it);
        self.label(before);
        self.i_load(it);
        self.a_load(arr);
        self.array_length();
        self.if_i_cmp_ge(after);
        self.a_load(arr);
        self.i_load(it);
        self.load_from_stack(val_t, val);
        handle!(val_t, self.i_a_store(), self.b_a_store(), self.a_a_store());
        self.i_inc(it, 1);
        self.goto(before);
        self.label(after);
        self.a_load(arr);
      }
      And => {
        let out_label = self.new_label();
        let false_label = self.new_label();
        self.expr(&mut binary.l);
        self.if_eq(false_label);
        self.expr(&mut binary.r);
        self.if_eq(false_label);
        self.bool_const(true);
        self.goto(out_label);
        self.label(false_label);
        self.bool_const(false);
        self.label(out_label);
      }
      Or => {
        let out_label = self.new_label();
        let true_label = self.new_label();
        self.expr(&mut binary.l);
        self.if_ne(true_label);
        self.expr(&mut binary.r);
        self.if_ne(true_label);
        self.bool_const(false);
        self.goto(out_label);
        self.label(true_label);
        self.bool_const(true);
        self.label(out_label);
      }
      _ => {
        self.expr(&mut binary.l);
        self.expr(&mut binary.r);
        match binary.op {
          Add => self.i_add(),
          Sub => self.i_sub(),
          Mul => self.i_mul(),
          Div => self.i_div(),
          Mod => self.i_rem(),
          BAnd => self.i_and(),
          BOr => self.i_or(),
          BXor => self.i_xor(),
          Shl => self.i_shl(),
          Shr => self.i_u_shr(),
          Le => cmp!(self, if_i_cmp_le),
          Lt => cmp!(self, if_i_cmp_lt),
          Ge => cmp!(self, if_i_cmp_ge),
          Gt => cmp!(self, if_i_cmp_gt),
          Eq => match binary.l.get_type() {
            SemanticType::Null | SemanticType::Object(_) => cmp!(self, if_a_cmp_eq),
            SemanticType::Basic(name) if name == &"string" => cmp!(self, if_a_cmp_eq),
            _ => cmp!(self, if_i_cmp_eq),
          }
          Ne => match binary.l.get_type() {
            SemanticType::Null | SemanticType::Object(_) => cmp!(self, if_a_cmp_ne),
            SemanticType::Basic(name) if name == &"string" => cmp!(self, if_a_cmp_ne),
            _ => cmp!(self, if_i_cmp_ne),
          }
          _ => unreachable!(),
        }
      }
    }
  }

  fn call(&mut self, call: &mut Call) {
    unsafe {
      if call.is_arr_len {
        self.expr(if let Some(owner) = &mut call.owner { owner } else { unreachable!() });
        self.array_length();
        return;
      }
      let method = &*call.method;
      if let Some(owner) = &mut call.owner { self.expr(owner); }
      for arg in &mut call.arg {
        self.expr(arg);
      }
      let argument_types: Vec<JavaType> = method.param.iter().map(|var_def| var_def.type_.to_java()).collect();
      let return_type = method.ret_t.to_java();
      if method.static_ {
        self.invoke_static((*method.class).name, method.name, &argument_types, &return_type);
      } else {
        self.invoke_virtual((*method.class).name, method.name, &argument_types[1..], &return_type);
      }
    }
  }

  fn print(&mut self, print: &mut Print) {
    for print in &mut print.print {
      self.get_static("java/lang/System", "out", &JavaType::Class("java/io/PrintStream"));
      self.expr(print);
      self.invoke_virtual("java/io/PrintStream", "print", &[print.get_type().to_java()], &JavaType::Void);
    }
  }

  fn this(&mut self, _this: &mut This) {
    self.a_load(0);
  }

  fn type_cast(&mut self, type_cast: &mut TypeCast) {
    self.expr(&mut type_cast.expr);
    self.check_cast(type_cast.name);
  }

  fn type_test(&mut self, type_test: &mut TypeTest) {
    self.expr(&mut type_test.expr);
    self.instance_of(type_test.name);
  }

  fn indexed(&mut self, indexed: &mut Indexed) {
    self.expr(&mut indexed.arr);
    self.expr(&mut indexed.idx);
    if !indexed.for_assign {
      handle!(&indexed.type_, self.i_a_load(), self.b_a_load(), self.a_a_load());
    }
  }

  fn identifier(&mut self, identifier: &mut Identifier) {
    unsafe {
      if !identifier.for_assign {
        match identifier.symbol {
          Var::VarDef(var_def) => {
            let var_def = &*var_def;
            match (*var_def.scope).kind {
              ScopeKind::Local(_) | ScopeKind::Parameter(_) => self.load_from_stack(&var_def.type_, var_def.index),
              ScopeKind::Class(class) => {
                self.expr(if let Some(owner) = &mut identifier.owner { owner } else { unreachable!() });
                self.get_field((*class).name, var_def.name, &var_def.type_.to_java())
              }
              _ => unreachable!(),
            }
          }
          Var::VarAssign(var_assign) => self.load_from_stack(&(*var_assign).type_, (*var_assign).index),
        }
      } else {
        if let Some(owner) = &mut identifier.owner { self.expr(owner); }
      }
    }
  }

  fn default(&mut self, default: &mut Default) {
    let arr = self.new_local();
    let dft = self.new_label();
    let after = self.new_label();
    self.expr(&mut default.arr);
    self.a_store(arr);
    self.expr(&mut default.idx);
    self.dup();
    self.if_lt(dft); // notice the difference between if_lt & if_i_cmp_lt
    self.dup();
    self.a_load(arr);
    self.array_length();
    self.if_i_cmp_ge(dft);
    self.dup();
    self.a_load(arr);
    self.swap();
    handle!(&default.type_, self.i_a_load(), self.b_a_load(), self.a_a_load());
    self.goto(after);
    self.label(dft);
    self.expr(&mut default.dft);
    self.label(after);
  }
}
