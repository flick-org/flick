use llvm_sys::analysis::LLVMVerifierFailureAction::LLVMPrintMessageAction;
use llvm_sys::analysis::LLVMVerifyFunction;
use std::ffi::{c_char, c_uint, c_ulonglong, CString};
use std::mem::MaybeUninit;
use std::process::Command;

use llvm_sys::core::*;
use llvm_sys::prelude::*;
use llvm_sys::target::{
    LLVMSetModuleDataLayout, LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter,
    LLVM_InitializeNativeTarget,
};
use llvm_sys::target_machine::LLVMCodeGenFileType::{LLVMAssemblyFile, LLVMObjectFile};
use llvm_sys::target_machine::LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault;
use llvm_sys::target_machine::LLVMCodeModel::LLVMCodeModelDefault;
use llvm_sys::target_machine::LLVMRelocMode::LLVMRelocDefault;
use llvm_sys::target_machine::{
    LLVMCreateTargetDataLayout, LLVMCreateTargetMachine, LLVMDisposeTargetMachine,
    LLVMGetDefaultTargetTriple, LLVMGetTargetFromTriple, LLVMTarget, LLVMTargetMachineEmitToFile,
    LLVMTargetMachineEmitToMemoryBuffer, LLVMTargetMachineRef,
};
use llvm_sys::transforms::ipo::LLVMAddFunctionInliningPass;
use llvm_sys::transforms::scalar::{
    LLVMAddCFGSimplificationPass, LLVMAddGVNPass, LLVMAddInstructionCombiningPass,
    LLVMAddReassociatePass,
};
use llvm_sys::LLVMIntPredicate::*;
use llvm_sys::LLVMLinkage::{LLVMExternalLinkage, LLVMInternalLinkage};

use crate::compiler::scope_manager::ScopeManager;
use crate::lexer::token::Type;
use crate::parser::ast::*;

macro_rules! cstr {
    ($str_literal:expr) => {
        concat!($str_literal, "\0").as_ptr() as *const _
    };
}

pub struct Compiler {
    context: LLVMContextRef,
    module: LLVMModuleRef,
    builder: LLVMBuilderRef,
    target_machine: LLVMTargetMachineRef,
    scope_manager: ScopeManager,
    pass_manager: LLVMPassManagerRef,
}

impl Compiler {
    unsafe fn get_target_from_triple(triple: *const c_char) -> *mut LLVMTarget {
        let mut target = std::ptr::null_mut();
        let mut err_str = MaybeUninit::uninit();
        if LLVMGetTargetFromTriple(triple, &mut target, err_str.as_mut_ptr()) != 0 {
            panic!(
                "Error getting target from triple ({:?})",
                err_str.assume_init()
            );
        }
        target
    }

    pub fn new() -> Self {
        unsafe {
            let context = LLVMContextCreate();
            let module = LLVMModuleCreateWithNameInContext(cstr!("module"), context);
            let builder = LLVMCreateBuilderInContext(context);
            let scope_manager = ScopeManager::new();

            // TODO: make this an option for the compiler to choose which target to initialize
            if LLVM_InitializeNativeTarget() == 1 {
                panic!("Error initializing native target")
            }
            if LLVM_InitializeNativeAsmParser() == 1 {
                panic!("Error initializing native ASM Parser")
            }
            if LLVM_InitializeNativeAsmPrinter() == 1 {
                panic!("Error initializing native ASM printer")
            }

            // Configure module
            let triple = LLVMGetDefaultTargetTriple(); // this computer's OS triple
            LLVMSetTarget(module, triple);

            let cpu = cstr!("generic");
            let features = cstr!("");
            let target = Self::get_target_from_triple(triple);
            let target_machine = LLVMCreateTargetMachine(
                target,
                triple,
                cpu,
                features,
                LLVMCodeGenLevelDefault,
                LLVMRelocDefault,
                LLVMCodeModelDefault,
            );

            let target_data_layout = LLVMCreateTargetDataLayout(target_machine);
            LLVMSetModuleDataLayout(module, target_data_layout);

            // Configure pass manager
            let pass_manager = LLVMCreatePassManager();

            LLVMAddFunctionInliningPass(pass_manager);
            LLVMAddInstructionCombiningPass(pass_manager);
            LLVMAddReassociatePass(pass_manager);
            LLVMAddGVNPass(pass_manager);
            LLVMAddCFGSimplificationPass(pass_manager);

            Self {
                context,
                module,
                builder,
                target_machine,
                scope_manager,
                pass_manager,
            }
        }
    }

    pub fn print_ir(&self) {
        unsafe { LLVMDumpModule(self.module) }
    }

    pub fn optimize(&mut self) {
        unsafe {
            if LLVMRunPassManager(self.pass_manager, self.module) != 1 {
                panic!("Error running optimizations");
            }
        }
    }

    // TODO: Better way of passing in path - maybe using AsRef?
    pub fn to_file(&self, path: String) {
        unsafe {
            let mut path_bytes = path.into_bytes();
            path_bytes.push(b'\0');
            let mut path_cchars: Vec<_> = path_bytes.iter().map(|b| *b as c_char).collect();

            let mut err_str = MaybeUninit::uninit();
            let result = LLVMTargetMachineEmitToFile(
                self.target_machine,
                self.module,
                path_cchars.as_mut_ptr(),
                LLVMObjectFile,
                err_str.as_mut_ptr(),
            );

            if result == 1 {
                panic!("Error emitting object file ({:?})", err_str.assume_init());
            }
        }
    }

    pub fn compile(&mut self, program: &Program) {
        unsafe {
            for func_def in program.func_defs.iter() {
                self.compile_func_def(func_def);
            }
        }
    }

    unsafe fn compile_func_def(&mut self, func_def: &FuncDef) {
        let func_name = CString::new(func_def.name.as_str()).unwrap();
        let func = LLVMGetNamedFunction(self.module, func_name.as_ptr());
        if !func.is_null() {
            panic!("Cannot redefine function '{}'", func_def.name);
        }

        // function does not yet exist, let's generate it:

        let return_type = self.to_llvm_type(func_def.return_type);
        let num_params = func_def.params.len() as c_uint;
        let mut param_types: Vec<_> = func_def
            .params
            .iter()
            .map(|p| self.to_llvm_type(p.param_type))
            .collect();

        let func_type = LLVMFunctionType(return_type, param_types.as_mut_ptr(), num_params, 0);

        let func = LLVMAddFunction(self.module, func_name.as_ptr(), func_type);

        match func_def.is_public {
            true => LLVMSetLinkage(func, LLVMExternalLinkage),
            false => LLVMSetLinkage(func, LLVMInternalLinkage),
        }

        if func.is_null() {
            panic!("Error defining function '{}'", func_def.name);
        }

        for (i, param) in func_def.params.iter().enumerate() {
            let param_value_ref = LLVMGetParam(func, i as c_uint);
            let param_name = CString::new(param.param_name.as_str()).unwrap();
            let param_name_len = param.param_name.len();
            LLVMSetValueName2(param_value_ref, param_name.as_ptr(), param_name_len);
        }

        let entry_block = LLVMAppendBasicBlockInContext(self.context, func, cstr!("entry"));
        LLVMPositionBuilderAtEnd(self.builder, entry_block);

        self.scope_manager.enter_scope();

        for (i, param) in func_def.params.iter().enumerate() {
            let param_name = param.param_name.as_str();
            let param_value_ref = LLVMGetParam(func, i as c_uint);
            let alloca = self.create_entry_block_alloca(func, param_name, param.param_type);
            LLVMBuildStore(self.builder, param_value_ref, alloca);
            self.scope_manager.set_var(param_name, alloca);
        }

        for statement in func_def.body.iter() {
            // TODO: Error if compiling statement fails?
            self.compile_statement(statement)
        }

        match func_def.body.last() {
            Some(Statement::Return(_)) => {}
            _ => self.compile_ret_statement(&None),
        }

        self.scope_manager.exit_scope();

        LLVMVerifyFunction(func, LLVMPrintMessageAction);
    }

    unsafe fn compile_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::VarDeclarations(v) => self.compile_var_declarations(v),
            Statement::WhileLoop(w) => self.compile_while_loop(w),
            Statement::Expr(e) => self.compile_expr_statement(e),
            Statement::Return(r) => self.compile_ret_statement(r),
        }
    }

    unsafe fn to_llvm_type(&self, t: Type) -> LLVMTypeRef {
        match t {
            Type::I64 => LLVMIntTypeInContext(self.context, 64),
            Type::Void => LLVMVoidTypeInContext(self.context),
        }
    }

    fn compile_while_loop(&self, while_loop: &WhileLoop) {
        todo!()
    }

    unsafe fn get_cur_function(&self) -> LLVMValueRef {
        LLVMGetBasicBlockParent(LLVMGetInsertBlock(self.builder))
    }

    unsafe fn compile_assignment(&mut self, assign: &Assign) -> LLVMValueRef {
        let value = self.compile_expr(assign.value.as_ref());
        let alloca = match self.scope_manager.get_var(assign.name.as_str()) {
            Some(alloca) => alloca,
            None => panic!("Setting a variable that has not been declared"),
        };
        LLVMBuildStore(self.builder, value, alloca);
        value
    }

    unsafe fn compile_var_declarations(&mut self, var_declarations: &[VarDeclaration]) {
        let func = self.get_cur_function();
        for var_declaration in var_declarations {
            let var_name = var_declaration.var_name.as_str();
            let var_type = var_declaration.var_type;
            let alloca = self.create_entry_block_alloca(func, var_name, var_type);
            self.scope_manager.set_var(var_name, alloca);

            if let Some(value_expr) = &var_declaration.var_value {
                let value = self.compile_expr(value_expr);
                LLVMBuildStore(self.builder, value, alloca);
            }
        }
    }

    unsafe fn compile_expr_statement(&mut self, expr: &Expr) {
        let _ = self.compile_expr(expr);
    }

    unsafe fn compile_expr(&mut self, expr: &Expr) -> LLVMValueRef {
        match expr {
            Expr::Identifier(id) => self.compile_identifier(id),
            Expr::I64Literal(x) => self.compile_i64_literal(*x),
            Expr::Binary(bin_expr) => self.compile_bin_expr(bin_expr),
            Expr::Call(call_expr) => self.compile_call_expr(call_expr),
            Expr::Assign(assign) => self.compile_assignment(assign),
        }
    }

    unsafe fn compile_ret_statement(&mut self, ret_value: &Option<Expr>) {
        // TODO once we set up type checking and once we can do
        // Expr::get_type(), we should make sure that ret_value matches
        // LLVMGETReturnType(self.cur_function())
        match ret_value {
            Some(expr) => LLVMBuildRet(self.builder, self.compile_expr(expr)),
            None => LLVMBuildRetVoid(self.builder),
        };
    }

    unsafe fn compile_identifier(&mut self, id: &str) -> LLVMValueRef {
        let alloca_ref = match self.scope_manager.get_var(id) {
            Some(alloca_ref) => alloca_ref,
            None => panic!("Compiler error: undefined identifier '{}'", id),
        };
        let alloca_type = LLVMGetAllocatedType(alloca_ref);
        let name = CString::new(id).unwrap();
        LLVMBuildLoad2(self.builder, alloca_type, alloca_ref, name.as_ptr())
    }

    unsafe fn compile_i64_literal(&self, x: i64) -> LLVMValueRef {
        LLVMConstInt(self.to_llvm_type(Type::I64), x as c_ulonglong, 1)
    }

    unsafe fn compile_bin_expr(&mut self, bin_expr: &Binary) -> LLVMValueRef {
        use BinaryOperator::*;

        let lhs = self.compile_expr(&bin_expr.left);
        let rhs = self.compile_expr(&bin_expr.right);

        match bin_expr.operator {
            Add => LLVMBuildAdd(self.builder, lhs, rhs, cstr!("add")),
            Subtract => LLVMBuildSub(self.builder, lhs, rhs, cstr!("sub")),
            Multiply => LLVMBuildMul(self.builder, lhs, rhs, cstr!("mul")),
            Divide => LLVMBuildSDiv(self.builder, lhs, rhs, cstr!("div")),

            NotEqualTo => LLVMBuildICmp(self.builder, LLVMIntNE, lhs, rhs, cstr!("neq")),
            EqualTo => LLVMBuildICmp(self.builder, LLVMIntEQ, lhs, rhs, cstr!("eq")),
            LessThan => LLVMBuildICmp(self.builder, LLVMIntSLT, lhs, rhs, cstr!("lt")),
            GreaterThan => LLVMBuildICmp(self.builder, LLVMIntSGT, lhs, rhs, cstr!("gt")),
            LessOrEqualTo => LLVMBuildICmp(self.builder, LLVMIntSLE, lhs, rhs, cstr!("lte")),
            GreaterOrEqualTo => LLVMBuildICmp(self.builder, LLVMIntSGE, lhs, rhs, cstr!("gte")),
        }
    }

    unsafe fn compile_call_expr(&mut self, call_expr: &Call) -> LLVMValueRef {
        let func_name = CString::new(call_expr.function_name.as_str()).unwrap();
        let func = LLVMGetNamedFunction(self.module, func_name.as_ptr());

        if func.is_null() {
            panic!("Unknown function '{}' referenced", call_expr.function_name);
        }

        let num_params = LLVMCountParams(func) as usize;
        if num_params != call_expr.args.len() {
            panic!(
                "Incorrect # arguments passed to function '{}'",
                call_expr.function_name
            );
        }

        // todo once type checking: compare arg and param types

        let mut arg_values: Vec<_> = call_expr
            .args
            .iter()
            .map(|expr| self.compile_expr(expr))
            .collect();

        // TODO: Add more useful error message on info abt WHAT argument is null
        if arg_values.iter().any(|value_ref| value_ref.is_null()) {
            panic!(
                "One of the arguments to function '{}' was null",
                call_expr.function_name
            )
        }

        let func_type = LLVMGlobalGetValueType(func);
        LLVMBuildCall2(
            self.builder,
            func_type,
            func,
            arg_values.as_mut_ptr(),
            arg_values.len() as c_uint,
            cstr!("call"),
        )
    }

    // TODO: Have this function get_cur_function instead of taking it as an argument
    unsafe fn create_entry_block_alloca(
        &self,
        func: LLVMValueRef,
        var_name: &str,
        var_type: Type,
    ) -> LLVMValueRef {
        // todo  reposition self.builder instead of creating temp builder
        let temp_builder = LLVMCreateBuilderInContext(self.context);
        let entry_block = LLVMGetEntryBasicBlock(func);
        LLVMPositionBuilderAtEnd(temp_builder, entry_block);
        let name = CString::new(var_name).unwrap();
        let alloca = LLVMBuildAlloca(temp_builder, self.to_llvm_type(var_type), name.as_ptr());
        LLVMDisposeBuilder(temp_builder);
        alloca
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        unsafe {
            LLVMDisposePassManager(self.pass_manager);
            LLVMDisposeTargetMachine(self.target_machine);
            LLVMDisposeBuilder(self.builder);
            LLVMDisposeModule(self.module);
            LLVMContextDispose(self.context);
        }
    }
}
