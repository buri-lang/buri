const $k0=[930n,'bg-t_app_bg'];
const $k1=[960n,'fg-t_app_fg'];
const $k2=[$k0,$k1];
const $k3=[$k2];
const $k4=[5,$k3];
const $k5=[$k4];
const $k6=[630n,'px-r0_5'];
const $k7=[1080n,'r-6'];
const $k8=[930n,'bg-t_cardlib_surface'];
const $k9=[960n,'fg-t_cardlib_onSurface'];
const $k10=[935n,'hover_bg-t_cardlib_danger'];
const $k11=[$k6,$k7,$k8,$k9,$k10];
const $k12=[$k11];
const $k13=[5,$k12];
const $k14=[$k13];
const $k15=[180n,'lay-row'];
const $k16=[$k15];
const $k17=[$k16];
const $k18=[5,$k17];
const $k19=[0,220n,38n,38n];
const $k20=[0,24n,24n,27n];
const $k21=[0,240n,240n,245n];
const $k22=[0,255n,255n,255n];
const $k23=[180n,'lay-col'];
const $k24=[$k23];
const $k25=[$k24];
const $k26=[5,$k25];
$ui_sheet='.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-t_app_bg{background-color:var(--app-bg)}\n.bg-t_cardlib_surface{background-color:var(--cardlib-surface)}\n.hover_bg-t_cardlib_danger:hover{background-color:var(--cardlib-danger)}\n.fg-t_app_fg{color:var(--app-fg)}\n.fg-t_cardlib_onSurface{color:var(--cardlib-onSurface)}\n.r-6{border-radius:6px}\n';
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const dark_1=[$host_HostUi_signal(ctx_0[2],false)];
  $host_HostStdout_println(ctx_0[1],'mounted');
  const label_7='clicks';
  const count_8=[$host_HostUi_signal(ctx_0[2],0n)];
  const count_24=[1,count_8];
  const content_32=[2,c_25=>{
    let $t1;
    if(count_24[0]===0){
      $t1=count_24[1];
    }else if(count_24[0]===1){
      $t1=ui_signal_lib_buri$Signal_get$io0sz4(count_24[1],c_25);
    }else if(count_24[0]===2){
      $t1=count_24[1](c_25);
    }else{
      $abort('no arm matched');
    }
    return String($t1);
  }];
  const $t6=ui_node_lib_buri$column$u3rqgv($k5,[[[5,[0,label_7],(c_9,e_10)=>$host_HostUi_write(c_9[2],count_8[0],(n_11=>n_11+1n)($host_HostUi_read(c_9[2],count_8[0])))]],[[3,[$k18,[0,$k14]],[[[1,[0,label_7]]],[[1,content_32]]]]]]);
  const $t5=ui_theme_lib_buri$themed([[__cmd_x_main_buri$Card_color(0),__cmd_x_main_buri$cardTheme(0)],[__cmd_x_main_buri$Card_color(1),__cmd_x_main_buri$cardTheme(1)],[__cmd_x_main_buri$Card_color(2),__cmd_x_main_buri$cardTheme(2)]]);
  const whenTrue_14=__cmd_x_main_buri$appThemed(__cmd_x_main_buri$night);
  const whenFalse_15=__cmd_x_main_buri$appThemed(__cmd_x_main_buri$day);
  return $ui_node_mount(ctx_0,$t6,[$t5,[[1,[1,dark_1],[whenTrue_14],[whenFalse_15]]]]);
}
function __cmd_x_main_buri$cardTheme(t_0){
  switch(t_0){
    case 0:
      {
        return __cmd_x_main_buri$App_color(0);
      }
    case 1:
      {
        return __cmd_x_main_buri$App_color(1);
      }
    case 2:
      {
        return $k19;
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function __cmd_x_main_buri$night(t_0){
  if(t_0===0){
    return $k20;
  }else if(t_0===1){
    return $k21;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$appThemed(f_0){
  const bindings_1=[[__cmd_x_main_buri$App_color(0),f_0(0)],[__cmd_x_main_buri$App_color(1),f_0(1)]];
  return [[0,bindings_1]];
}
function __cmd_x_main_buri$day(t_0){
  if(t_0===0){
    return $k22;
  }else if(t_0===1){
    return $k20;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$App_color(self_0){
  if(self_0===0){
    return [2,['app','bg']];
  }else if(self_0===1){
    return [2,['app','fg']];
  }else{
    $abort('no arm matched');
  }
}
function ui_theme_lib_buri$themed(bindings_0){
  return [[0,bindings_0]];
}
function __cmd_x_main_buri$Card_color(self_0){
  switch(self_0){
    case 0:
      {
        return [2,['cardlib','surface']];
      }
    case 1:
      {
        return [2,['cardlib','onSurface']];
      }
    case 2:
      {
        return [2,['cardlib','danger']];
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_node_lib_buri$column$u3rqgv(styles_0,children_1){
  return [[3,[$k26,[0,styles_0]],children_1]];
}
function ui_signal_lib_buri$Signal_get$io0sz4(self_0,ctx_1){
  return $ui_effect_Scope_read(ctx_1,self_0[0]);
}
