import{a9 as ys,aa as Ts,V as bs,$ as L,e as Ct,h as W,a as Xa,ab as Nt,ac as Ss,ad as Tt,y as Se,ae as Ba,af as Xt,ag as Cs,ah as be,Z as Qt,f as ws,a4 as As,a5 as Ns}from"./Bqb1CCxh.js";var Je={exports:{}};var Xe=typeof window<"u"&&typeof document<"u"&&typeof navigator<"u",Os=(function(){for(var a=["Edge","Trident","Firefox"],n=0;n<a.length;n+=1)if(Xe&&navigator.userAgent.indexOf(a[n])>=0)return 1;return 0})();function Ds(a){var n=!1;return function(){n||(n=!0,window.Promise.resolve().then(function(){n=!1,a()}))}}function ks(a){var n=!1;return function(){n||(n=!0,setTimeout(function(){n=!1,a()},Os))}}var Is=Xe&&window.Promise,Ms=Is?Ds:ks;function Za(a){var n={};return a&&n.toString.call(a)==="[object Function]"}function oe(a,n){if(a.nodeType!==1)return[];var s=a.ownerDocument.defaultView,d=s.getComputedStyle(a,null);return n?d[n]:d}function Zt(a){return a.nodeName==="HTML"?a:a.parentNode||a.host}function Ze(a){if(!a)return document.body;switch(a.nodeName){case"HTML":case"BODY":return a.ownerDocument.body;case"#document":return a.body}var n=oe(a),s=n.overflow,d=n.overflowX,h=n.overflowY;return/(auto|scroll|overlay)/.test(s+h+d)?a:Ze(Zt(a))}function en(a){return a&&a.referenceNode?a.referenceNode:a}var Ka=Xe&&!!(window.MSInputMethodContext&&document.documentMode),Ya=Xe&&/MSIE 10/.test(navigator.userAgent);function Ne(a){return a===11?Ka:a===10?Ya:Ka||Ya}function Ce(a){if(!a)return document.documentElement;for(var n=Ne(10)?document.body:null,s=a.offsetParent||null;s===n&&a.nextElementSibling;)s=(a=a.nextElementSibling).offsetParent;var d=s&&s.nodeName;return!d||d==="BODY"||d==="HTML"?a?a.ownerDocument.documentElement:document.documentElement:["TH","TD","TABLE"].indexOf(s.nodeName)!==-1&&oe(s,"position")==="static"?Ce(s):s}function Ls(a){var n=a.nodeName;return n==="BODY"?!1:n==="HTML"||Ce(a.firstElementChild)===a}function Ft(a){return a.parentNode!==null?Ft(a.parentNode):a}function wt(a,n){if(!a||!a.nodeType||!n||!n.nodeType)return document.documentElement;var s=a.compareDocumentPosition(n)&Node.DOCUMENT_POSITION_FOLLOWING,d=s?a:n,h=s?n:a,m=document.createRange();m.setStart(d,0),m.setEnd(h,0);var t=m.commonAncestorContainer;if(a!==t&&n!==t||d.contains(h))return Ls(t)?t:Ce(t);var r=Ft(a);return r.host?wt(r.host,n):wt(a,Ft(n).host)}function we(a){var n=arguments.length>1&&arguments[1]!==void 0?arguments[1]:"top",s=n==="top"?"scrollTop":"scrollLeft",d=a.nodeName;if(d==="BODY"||d==="HTML"){var h=a.ownerDocument.documentElement,m=a.ownerDocument.scrollingElement||h;return m[s]}return a[s]}function Rs(a,n){var s=arguments.length>2&&arguments[2]!==void 0?arguments[2]:!1,d=we(n,"top"),h=we(n,"left"),m=s?-1:1;return a.top+=d*m,a.bottom+=d*m,a.left+=h*m,a.right+=h*m,a}function Ga(a,n){var s=n==="x"?"Left":"Top",d=s==="Left"?"Right":"Bottom";return parseFloat(a["border"+s+"Width"])+parseFloat(a["border"+d+"Width"])}function qa(a,n,s,d){return Math.max(n["offset"+a],n["scroll"+a],s["client"+a],s["offset"+a],s["scroll"+a],Ne(10)?parseInt(s["offset"+a])+parseInt(d["margin"+(a==="Height"?"Top":"Left")])+parseInt(d["margin"+(a==="Height"?"Bottom":"Right")]):0)}function tn(a){var n=a.body,s=a.documentElement,d=Ne(10)&&getComputedStyle(s);return{height:qa("Height",n,s,d),width:qa("Width",n,s,d)}}var xs=function(a,n){if(!(a instanceof n))throw new TypeError("Cannot call a class as a function")},Ps=(function(){function a(n,s){for(var d=0;d<s.length;d++){var h=s[d];h.enumerable=h.enumerable||!1,h.configurable=!0,"value"in h&&(h.writable=!0),Object.defineProperty(n,h.key,h)}}return function(n,s,d){return s&&a(n.prototype,s),d&&a(n,d),n}})(),Ae=function(a,n,s){return n in a?Object.defineProperty(a,n,{value:s,enumerable:!0,configurable:!0,writable:!0}):a[n]=s,a},B=Object.assign||function(a){for(var n=1;n<arguments.length;n++){var s=arguments[n];for(var d in s)Object.prototype.hasOwnProperty.call(s,d)&&(a[d]=s[d])}return a};function ee(a){return B({},a,{right:a.left+a.width,bottom:a.top+a.height})}function zt(a){var n={};try{if(Ne(10)){n=a.getBoundingClientRect();var s=we(a,"top"),d=we(a,"left");n.top+=s,n.left+=d,n.bottom+=s,n.right+=d}else n=a.getBoundingClientRect()}catch{}var h={left:n.left,top:n.top,width:n.right-n.left,height:n.bottom-n.top},m=a.nodeName==="HTML"?tn(a.ownerDocument):{},t=m.width||a.clientWidth||h.width,r=m.height||a.clientHeight||h.height,S=a.offsetWidth-t,b=a.offsetHeight-r;if(S||b){var y=oe(a);S-=Ga(y,"x"),b-=Ga(y,"y"),h.width-=S,h.height-=b}return ee(h)}function ea(a,n){var s=arguments.length>2&&arguments[2]!==void 0?arguments[2]:!1,d=Ne(10),h=n.nodeName==="HTML",m=zt(a),t=zt(n),r=Ze(a),S=oe(n),b=parseFloat(S.borderTopWidth),y=parseFloat(S.borderLeftWidth);s&&h&&(t.top=Math.max(t.top,0),t.left=Math.max(t.left,0));var c=ee({top:m.top-t.top-b,left:m.left-t.left-y,width:m.width,height:m.height});if(c.marginTop=0,c.marginLeft=0,!d&&h){var v=parseFloat(S.marginTop),f=parseFloat(S.marginLeft);c.top-=b-v,c.bottom-=b-v,c.left-=y-f,c.right-=y-f,c.marginTop=v,c.marginLeft=f}return(d&&!s?n.contains(r):n===r&&r.nodeName!=="BODY")&&(c=Rs(c,n)),c}function js(a){var n=arguments.length>1&&arguments[1]!==void 0?arguments[1]:!1,s=a.ownerDocument.documentElement,d=ea(a,s),h=Math.max(s.clientWidth,window.innerWidth||0),m=Math.max(s.clientHeight,window.innerHeight||0),t=n?0:we(s),r=n?0:we(s,"left"),S={top:t-d.top+d.marginTop,left:r-d.left+d.marginLeft,width:h,height:m};return ee(S)}function an(a){var n=a.nodeName;if(n==="BODY"||n==="HTML")return!1;if(oe(a,"position")==="fixed")return!0;var s=Zt(a);return s?an(s):!1}function nn(a){if(!a||!a.parentElement||Ne())return document.documentElement;for(var n=a.parentElement;n&&oe(n,"transform")==="none";)n=n.parentElement;return n||document.documentElement}function ta(a,n,s,d){var h=arguments.length>4&&arguments[4]!==void 0?arguments[4]:!1,m={top:0,left:0},t=h?nn(a):wt(a,en(n));if(d==="viewport")m=js(t,h);else{var r=void 0;d==="scrollParent"?(r=Ze(Zt(n)),r.nodeName==="BODY"&&(r=a.ownerDocument.documentElement)):d==="window"?r=a.ownerDocument.documentElement:r=d;var S=ea(r,t,h);if(r.nodeName==="HTML"&&!an(t)){var b=tn(a.ownerDocument),y=b.height,c=b.width;m.top+=S.top-S.marginTop,m.bottom=y+S.top,m.left+=S.left-S.marginLeft,m.right=c+S.left}else m=S}s=s||0;var v=typeof s=="number";return m.left+=v?s:s.left||0,m.top+=v?s:s.top||0,m.right-=v?s:s.right||0,m.bottom-=v?s:s.bottom||0,m}function Hs(a){var n=a.width,s=a.height;return n*s}function rn(a,n,s,d,h){var m=arguments.length>5&&arguments[5]!==void 0?arguments[5]:0;if(a.indexOf("auto")===-1)return a;var t=ta(s,d,m,h),r={top:{width:t.width,height:n.top-t.top},right:{width:t.right-n.right,height:t.height},bottom:{width:t.width,height:t.bottom-n.bottom},left:{width:n.left-t.left,height:t.height}},S=Object.keys(r).map(function(v){return B({key:v},r[v],{area:Hs(r[v])})}).sort(function(v,f){return f.area-v.area}),b=S.filter(function(v){var f=v.width,_=v.height;return f>=s.clientWidth&&_>=s.clientHeight}),y=b.length>0?b[0].key:S[0].key,c=a.split("-")[1];return y+(c?"-"+c:"")}function sn(a,n,s){var d=arguments.length>3&&arguments[3]!==void 0?arguments[3]:null,h=d?nn(n):wt(n,en(s));return ea(s,h,d)}function ln(a){var n=a.ownerDocument.defaultView,s=n.getComputedStyle(a),d=parseFloat(s.marginTop||0)+parseFloat(s.marginBottom||0),h=parseFloat(s.marginLeft||0)+parseFloat(s.marginRight||0),m={width:a.offsetWidth+h,height:a.offsetHeight+d};return m}function At(a){var n={left:"right",right:"left",bottom:"top",top:"bottom"};return a.replace(/left|right|bottom|top/g,function(s){return n[s]})}function on(a,n,s){s=s.split("-")[0];var d=ln(a),h={width:d.width,height:d.height},m=["right","left"].indexOf(s)!==-1,t=m?"top":"left",r=m?"left":"top",S=m?"height":"width",b=m?"width":"height";return h[t]=n[t]+n[S]/2-d[S]/2,s===r?h[r]=n[r]-d[b]:h[r]=n[At(r)],h}function et(a,n){return Array.prototype.find?a.find(n):a.filter(n)[0]}function Vs(a,n,s){if(Array.prototype.findIndex)return a.findIndex(function(h){return h[n]===s});var d=et(a,function(h){return h[n]===s});return a.indexOf(d)}function dn(a,n,s){var d=s===void 0?a:a.slice(0,Vs(a,"name",s));return d.forEach(function(h){h.function&&console.warn("`modifier.function` is deprecated, use `modifier.fn`!");var m=h.function||h.fn;h.enabled&&Za(m)&&(n.offsets.popper=ee(n.offsets.popper),n.offsets.reference=ee(n.offsets.reference),n=m(n,h))}),n}function $s(){if(!this.state.isDestroyed){var a={instance:this,styles:{},arrowStyles:{},attributes:{},flipped:!1,offsets:{}};a.offsets.reference=sn(this.state,this.popper,this.reference,this.options.positionFixed),a.placement=rn(this.options.placement,a.offsets.reference,this.popper,this.reference,this.options.modifiers.flip.boundariesElement,this.options.modifiers.flip.padding),a.originalPlacement=a.placement,a.positionFixed=this.options.positionFixed,a.offsets.popper=on(this.popper,a.offsets.reference,a.placement),a.offsets.popper.position=this.options.positionFixed?"fixed":"absolute",a=dn(this.modifiers,a),this.state.isCreated?this.options.onUpdate(a):(this.state.isCreated=!0,this.options.onCreate(a))}}function cn(a,n){return a.some(function(s){var d=s.name,h=s.enabled;return h&&d===n})}function aa(a){for(var n=[!1,"ms","Webkit","Moz","O"],s=a.charAt(0).toUpperCase()+a.slice(1),d=0;d<n.length;d++){var h=n[d],m=h?""+h+s:a;if(typeof document.body.style[m]<"u")return m}return null}function Us(){return this.state.isDestroyed=!0,cn(this.modifiers,"applyStyle")&&(this.popper.removeAttribute("x-placement"),this.popper.style.position="",this.popper.style.top="",this.popper.style.left="",this.popper.style.right="",this.popper.style.bottom="",this.popper.style.willChange="",this.popper.style[aa("transform")]=""),this.disableEventListeners(),this.options.removeOnDestroy&&this.popper.parentNode.removeChild(this.popper),this}function un(a){var n=a.ownerDocument;return n?n.defaultView:window}function fn(a,n,s,d){var h=a.nodeName==="BODY",m=h?a.ownerDocument.defaultView:a;m.addEventListener(n,s,{passive:!0}),h||fn(Ze(m.parentNode),n,s,d),d.push(m)}function Ws(a,n,s,d){s.updateBound=d,un(a).addEventListener("resize",s.updateBound,{passive:!0});var h=Ze(a);return fn(h,"scroll",s.updateBound,s.scrollParents),s.scrollElement=h,s.eventsEnabled=!0,s}function Bs(){this.state.eventsEnabled||(this.state=Ws(this.reference,this.options,this.state,this.scheduleUpdate))}function Ks(a,n){return un(a).removeEventListener("resize",n.updateBound),n.scrollParents.forEach(function(s){s.removeEventListener("scroll",n.updateBound)}),n.updateBound=null,n.scrollParents=[],n.scrollElement=null,n.eventsEnabled=!1,n}function Ys(){this.state.eventsEnabled&&(cancelAnimationFrame(this.scheduleUpdate),this.state=Ks(this.reference,this.state))}function na(a){return a!==""&&!isNaN(parseFloat(a))&&isFinite(a)}function Jt(a,n){Object.keys(n).forEach(function(s){var d="";["width","height","top","right","bottom","left"].indexOf(s)!==-1&&na(n[s])&&(d="px"),a.style[s]=n[s]+d})}function Gs(a,n){Object.keys(n).forEach(function(s){var d=n[s];d!==!1?a.setAttribute(s,n[s]):a.removeAttribute(s)})}function qs(a){return Jt(a.instance.popper,a.styles),Gs(a.instance.popper,a.attributes),a.arrowElement&&Object.keys(a.arrowStyles).length&&Jt(a.arrowElement,a.arrowStyles),a}function Qs(a,n,s,d,h){var m=sn(h,n,a,s.positionFixed),t=rn(s.placement,m,n,a,s.modifiers.flip.boundariesElement,s.modifiers.flip.padding);return n.setAttribute("x-placement",t),Jt(n,{position:s.positionFixed?"fixed":"absolute"}),s}function Fs(a,n){var s=a.offsets,d=s.popper,h=s.reference,m=Math.round,t=Math.floor,r=function(k){return k},S=m(h.width),b=m(d.width),y=["left","right"].indexOf(a.placement)!==-1,c=a.placement.indexOf("-")!==-1,v=S%2===b%2,f=S%2===1&&b%2===1,_=n?y||c||v?m:t:r,A=n?m:r;return{left:_(f&&!c&&n?d.left-1:d.left),top:A(d.top),bottom:A(d.bottom),right:_(d.right)}}var zs=Xe&&/Firefox/i.test(navigator.userAgent);function Js(a,n){var s=n.x,d=n.y,h=a.offsets.popper,m=et(a.instance.modifiers,function(D){return D.name==="applyStyle"}).gpuAcceleration;m!==void 0&&console.warn("WARNING: `gpuAcceleration` option moved to `computeStyle` modifier and will not be supported in future versions of Popper.js!");var t=m!==void 0?m:n.gpuAcceleration,r=Ce(a.instance.popper),S=zt(r),b={position:h.position},y=Fs(a,window.devicePixelRatio<2||!zs),c=s==="bottom"?"top":"bottom",v=d==="right"?"left":"right",f=aa("transform"),_=void 0,A=void 0;if(c==="bottom"?r.nodeName==="HTML"?A=-r.clientHeight+y.bottom:A=-S.height+y.bottom:A=y.top,v==="right"?r.nodeName==="HTML"?_=-r.clientWidth+y.right:_=-S.width+y.right:_=y.left,t&&f)b[f]="translate3d("+_+"px, "+A+"px, 0)",b[c]=0,b[v]=0,b.willChange="transform";else{var O=c==="bottom"?-1:1,k=v==="right"?-1:1;b[c]=A*O,b[v]=_*k,b.willChange=c+", "+v}var w={"x-placement":a.placement};return a.attributes=B({},w,a.attributes),a.styles=B({},b,a.styles),a.arrowStyles=B({},a.offsets.arrow,a.arrowStyles),a}function hn(a,n,s){var d=et(a,function(r){var S=r.name;return S===n}),h=!!d&&a.some(function(r){return r.name===s&&r.enabled&&r.order<d.order});if(!h){var m="`"+n+"`",t="`"+s+"`";console.warn(t+" modifier is required by "+m+" modifier in order to work, be sure to include it before "+m+"!")}return h}function Xs(a,n){var s;if(!hn(a.instance.modifiers,"arrow","keepTogether"))return a;var d=n.element;if(typeof d=="string"){if(d=a.instance.popper.querySelector(d),!d)return a}else if(!a.instance.popper.contains(d))return console.warn("WARNING: `arrow.element` must be child of its popper element!"),a;var h=a.placement.split("-")[0],m=a.offsets,t=m.popper,r=m.reference,S=["left","right"].indexOf(h)!==-1,b=S?"height":"width",y=S?"Top":"Left",c=y.toLowerCase(),v=S?"left":"top",f=S?"bottom":"right",_=ln(d)[b];r[f]-_<t[c]&&(a.offsets.popper[c]-=t[c]-(r[f]-_)),r[c]+_>t[f]&&(a.offsets.popper[c]+=r[c]+_-t[f]),a.offsets.popper=ee(a.offsets.popper);var A=r[c]+r[b]/2-_/2,O=oe(a.instance.popper),k=parseFloat(O["margin"+y]),w=parseFloat(O["border"+y+"Width"]),D=A-a.offsets.popper[c]-k-w;return D=Math.max(Math.min(t[b]-_,D),0),a.arrowElement=d,a.offsets.arrow=(s={},Ae(s,c,Math.round(D)),Ae(s,v,""),s),a}function Zs(a){return a==="end"?"start":a==="start"?"end":a}var mn=["auto-start","auto","auto-end","top-start","top","top-end","right-start","right","right-end","bottom-end","bottom","bottom-start","left-end","left","left-start"],Yt=mn.slice(3);function Qa(a){var n=arguments.length>1&&arguments[1]!==void 0?arguments[1]:!1,s=Yt.indexOf(a),d=Yt.slice(s+1).concat(Yt.slice(0,s));return n?d.reverse():d}var Gt={FLIP:"flip",CLOCKWISE:"clockwise",COUNTERCLOCKWISE:"counterclockwise"};function el(a,n){if(cn(a.instance.modifiers,"inner")||a.flipped&&a.placement===a.originalPlacement)return a;var s=ta(a.instance.popper,a.instance.reference,n.padding,n.boundariesElement,a.positionFixed),d=a.placement.split("-")[0],h=At(d),m=a.placement.split("-")[1]||"",t=[];switch(n.behavior){case Gt.FLIP:t=[d,h];break;case Gt.CLOCKWISE:t=Qa(d);break;case Gt.COUNTERCLOCKWISE:t=Qa(d,!0);break;default:t=n.behavior}return t.forEach(function(r,S){if(d!==r||t.length===S+1)return a;d=a.placement.split("-")[0],h=At(d);var b=a.offsets.popper,y=a.offsets.reference,c=Math.floor,v=d==="left"&&c(b.right)>c(y.left)||d==="right"&&c(b.left)<c(y.right)||d==="top"&&c(b.bottom)>c(y.top)||d==="bottom"&&c(b.top)<c(y.bottom),f=c(b.left)<c(s.left),_=c(b.right)>c(s.right),A=c(b.top)<c(s.top),O=c(b.bottom)>c(s.bottom),k=d==="left"&&f||d==="right"&&_||d==="top"&&A||d==="bottom"&&O,w=["top","bottom"].indexOf(d)!==-1,D=!!n.flipVariations&&(w&&m==="start"&&f||w&&m==="end"&&_||!w&&m==="start"&&A||!w&&m==="end"&&O),g=!!n.flipVariationsByContent&&(w&&m==="start"&&_||w&&m==="end"&&f||!w&&m==="start"&&O||!w&&m==="end"&&A),I=D||g;(v||k||I)&&(a.flipped=!0,(v||k)&&(d=t[S+1]),I&&(m=Zs(m)),a.placement=d+(m?"-"+m:""),a.offsets.popper=B({},a.offsets.popper,on(a.instance.popper,a.offsets.reference,a.placement)),a=dn(a.instance.modifiers,a,"flip"))}),a}function tl(a){var n=a.offsets,s=n.popper,d=n.reference,h=a.placement.split("-")[0],m=Math.floor,t=["top","bottom"].indexOf(h)!==-1,r=t?"right":"bottom",S=t?"left":"top",b=t?"width":"height";return s[r]<m(d[S])&&(a.offsets.popper[S]=m(d[S])-s[b]),s[S]>m(d[r])&&(a.offsets.popper[S]=m(d[r])),a}function al(a,n,s,d){var h=a.match(/((?:\-|\+)?\d*\.?\d*)(.*)/),m=+h[1],t=h[2];if(!m)return a;if(t.indexOf("%")===0){var r=void 0;t==="%p"?r=s:r=d;var S=ee(r);return S[n]/100*m}else if(t==="vh"||t==="vw"){var b=void 0;return t==="vh"?b=Math.max(document.documentElement.clientHeight,window.innerHeight||0):b=Math.max(document.documentElement.clientWidth,window.innerWidth||0),b/100*m}else return m}function nl(a,n,s,d){var h=[0,0],m=["right","left"].indexOf(d)!==-1,t=a.split(/(\+|\-)/).map(function(y){return y.trim()}),r=t.indexOf(et(t,function(y){return y.search(/,|\s/)!==-1}));t[r]&&t[r].indexOf(",")===-1&&console.warn("Offsets separated by white space(s) are deprecated, use a comma (,) instead.");var S=/\s*,\s*|\s+/,b=r!==-1?[t.slice(0,r).concat([t[r].split(S)[0]]),[t[r].split(S)[1]].concat(t.slice(r+1))]:[t];return b=b.map(function(y,c){var v=(c===1?!m:m)?"height":"width",f=!1;return y.reduce(function(_,A){return _[_.length-1]===""&&["+","-"].indexOf(A)!==-1?(_[_.length-1]=A,f=!0,_):f?(_[_.length-1]+=A,f=!1,_):_.concat(A)},[]).map(function(_){return al(_,v,n,s)})}),b.forEach(function(y,c){y.forEach(function(v,f){na(v)&&(h[c]+=v*(y[f-1]==="-"?-1:1))})}),h}function il(a,n){var s=n.offset,d=a.placement,h=a.offsets,m=h.popper,t=h.reference,r=d.split("-")[0],S=void 0;return na(+s)?S=[+s,0]:S=nl(s,m,t,r),r==="left"?(m.top+=S[0],m.left-=S[1]):r==="right"?(m.top+=S[0],m.left+=S[1]):r==="top"?(m.left+=S[0],m.top-=S[1]):r==="bottom"&&(m.left+=S[0],m.top+=S[1]),a.popper=m,a}function rl(a,n){var s=n.boundariesElement||Ce(a.instance.popper);a.instance.reference===s&&(s=Ce(s));var d=aa("transform"),h=a.instance.popper.style,m=h.top,t=h.left,r=h[d];h.top="",h.left="",h[d]="";var S=ta(a.instance.popper,a.instance.reference,n.padding,s,a.positionFixed);h.top=m,h.left=t,h[d]=r,n.boundaries=S;var b=n.priority,y=a.offsets.popper,c={primary:function(f){var _=y[f];return y[f]<S[f]&&!n.escapeWithReference&&(_=Math.max(y[f],S[f])),Ae({},f,_)},secondary:function(f){var _=f==="right"?"left":"top",A=y[_];return y[f]>S[f]&&!n.escapeWithReference&&(A=Math.min(y[_],S[f]-(f==="right"?y.width:y.height))),Ae({},_,A)}};return b.forEach(function(v){var f=["left","top"].indexOf(v)!==-1?"primary":"secondary";y=B({},y,c[f](v))}),a.offsets.popper=y,a}function sl(a){var n=a.placement,s=n.split("-")[0],d=n.split("-")[1];if(d){var h=a.offsets,m=h.reference,t=h.popper,r=["bottom","top"].indexOf(s)!==-1,S=r?"left":"top",b=r?"width":"height",y={start:Ae({},S,m[S]),end:Ae({},S,m[S]+m[b]-t[b])};a.offsets.popper=B({},t,y[d])}return a}function ll(a){if(!hn(a.instance.modifiers,"hide","preventOverflow"))return a;var n=a.offsets.reference,s=et(a.instance.modifiers,function(d){return d.name==="preventOverflow"}).boundaries;if(n.bottom<s.top||n.left>s.right||n.top>s.bottom||n.right<s.left){if(a.hide===!0)return a;a.hide=!0,a.attributes["x-out-of-boundaries"]=""}else{if(a.hide===!1)return a;a.hide=!1,a.attributes["x-out-of-boundaries"]=!1}return a}function ol(a){var n=a.placement,s=n.split("-")[0],d=a.offsets,h=d.popper,m=d.reference,t=["left","right"].indexOf(s)!==-1,r=["top","left"].indexOf(s)===-1;return h[t?"left":"top"]=m[s]-(r?h[t?"width":"height"]:0),a.placement=At(n),a.offsets.popper=ee(h),a}var dl={shift:{order:100,enabled:!0,fn:sl},offset:{order:200,enabled:!0,fn:il,offset:0},preventOverflow:{order:300,enabled:!0,fn:rl,priority:["left","right","top","bottom"],padding:5,boundariesElement:"scrollParent"},keepTogether:{order:400,enabled:!0,fn:tl},arrow:{order:500,enabled:!0,fn:Xs,element:"[x-arrow]"},flip:{order:600,enabled:!0,fn:el,behavior:"flip",padding:5,boundariesElement:"viewport",flipVariations:!1,flipVariationsByContent:!1},inner:{order:700,enabled:!1,fn:ol},hide:{order:800,enabled:!0,fn:ll},computeStyle:{order:850,enabled:!0,fn:Js,gpuAcceleration:!0,x:"bottom",y:"right"},applyStyle:{order:900,enabled:!0,fn:qs,onLoad:Qs,gpuAcceleration:void 0}},cl={placement:"bottom",positionFixed:!1,eventsEnabled:!0,removeOnDestroy:!1,onCreate:function(){},onUpdate:function(){},modifiers:dl},Ot=(function(){function a(n,s){var d=this,h=arguments.length>2&&arguments[2]!==void 0?arguments[2]:{};xs(this,a),this.scheduleUpdate=function(){return requestAnimationFrame(d.update)},this.update=Ms(this.update.bind(this)),this.options=B({},a.Defaults,h),this.state={isDestroyed:!1,isCreated:!1,scrollParents:[]},this.reference=n&&n.jquery?n[0]:n,this.popper=s&&s.jquery?s[0]:s,this.options.modifiers={},Object.keys(B({},a.Defaults.modifiers,h.modifiers)).forEach(function(t){d.options.modifiers[t]=B({},a.Defaults.modifiers[t]||{},h.modifiers?h.modifiers[t]:{})}),this.modifiers=Object.keys(this.options.modifiers).map(function(t){return B({name:t},d.options.modifiers[t])}).sort(function(t,r){return t.order-r.order}),this.modifiers.forEach(function(t){t.enabled&&Za(t.onLoad)&&t.onLoad(d.reference,d.popper,d.options,t,d.state)}),this.update();var m=this.options.eventsEnabled;m&&this.enableEventListeners(),this.state.eventsEnabled=m}return Ps(a,[{key:"update",value:function(){return $s.call(this)}},{key:"destroy",value:function(){return Us.call(this)}},{key:"enableEventListeners",value:function(){return Bs.call(this)}},{key:"disableEventListeners",value:function(){return Ys.call(this)}}]),a})();Ot.Utils=(typeof window<"u"?window:global).PopperUtils;Ot.placements=mn;Ot.Defaults=cl;const ul=Object.freeze(Object.defineProperty({__proto__:null,default:Ot},Symbol.toStringTag,{value:"Module"})),fl=ys(ul);var hl=Je.exports,Fa;function ml(){return Fa||(Fa=1,(function(a,n){(function(s,d){d(n,Ts(),fl)})(hl,(function(s,d,h){function m(p){return p&&typeof p=="object"&&"default"in p?p:{default:p}}var t=m(d),r=m(h);function S(p,u){for(var o=0;o<u.length;o++){var e=u[o];e.enumerable=e.enumerable||!1,e.configurable=!0,"value"in e&&(e.writable=!0),Object.defineProperty(p,e.key,e)}}function b(p,u,o){return o&&S(p,o),Object.defineProperty(p,"prototype",{writable:!1}),p}function y(){return y=Object.assign?Object.assign.bind():function(p){for(var u=1;u<arguments.length;u++){var o=arguments[u];for(var e in o)Object.prototype.hasOwnProperty.call(o,e)&&(p[e]=o[e])}return p},y.apply(this,arguments)}function c(p,u){p.prototype=Object.create(u.prototype),p.prototype.constructor=p,v(p,u)}function v(p,u){return v=Object.setPrototypeOf?Object.setPrototypeOf.bind():function(e,i){return e.__proto__=i,e},v(p,u)}var f="transitionend",_=1e6,A=1e3;function O(p){return p===null||typeof p>"u"?""+p:{}.toString.call(p).match(/\s([a-z]+)/i)[1].toLowerCase()}function k(){return{bindType:f,delegateType:f,handle:function(u){if(t.default(u.target).is(this))return u.handleObj.handler.apply(this,arguments)}}}function w(p){var u=this,o=!1;return t.default(this).one(g.TRANSITION_END,function(){o=!0}),setTimeout(function(){o||g.triggerTransitionEnd(u)},p),this}function D(){t.default.fn.emulateTransitionEnd=w,t.default.event.special[g.TRANSITION_END]=k()}var g={TRANSITION_END:"bsTransitionEnd",getUID:function(u){do u+=~~(Math.random()*_);while(document.getElementById(u));return u},getSelectorFromElement:function(u){var o=u.getAttribute("data-target");if(!o||o==="#"){var e=u.getAttribute("href");o=e&&e!=="#"?e.trim():""}try{return document.querySelector(o)?o:null}catch{return null}},getTransitionDurationFromElement:function(u){if(!u)return 0;var o=t.default(u).css("transition-duration"),e=t.default(u).css("transition-delay"),i=parseFloat(o),l=parseFloat(e);return!i&&!l?0:(o=o.split(",")[0],e=e.split(",")[0],(parseFloat(o)+parseFloat(e))*A)},reflow:function(u){return u.offsetHeight},triggerTransitionEnd:function(u){t.default(u).trigger(f)},supportsTransitionEnd:function(){return!!f},isElement:function(u){return(u[0]||u).nodeType},typeCheckConfig:function(u,o,e){for(var i in e)if(Object.prototype.hasOwnProperty.call(e,i)){var l=e[i],E=o[i],T=E&&g.isElement(E)?"element":O(E);if(!new RegExp(l).test(T))throw new Error(u.toUpperCase()+": "+('Option "'+i+'" provided type "'+T+'" ')+('but expected type "'+l+'".'))}},findShadowRoot:function(u){if(!document.documentElement.attachShadow)return null;if(typeof u.getRootNode=="function"){var o=u.getRootNode();return o instanceof ShadowRoot?o:null}return u instanceof ShadowRoot?u:u.parentNode?g.findShadowRoot(u.parentNode):null},jQueryDetection:function(){if(typeof t.default>"u")throw new TypeError("Bootstrap's JavaScript requires jQuery. jQuery must be included before Bootstrap's JavaScript.");var u=t.default.fn.jquery.split(" ")[0].split("."),o=1,e=2,i=9,l=1,E=4;if(u[0]<e&&u[1]<i||u[0]===o&&u[1]===i&&u[2]<l||u[0]>=E)throw new Error("Bootstrap's JavaScript requires at least jQuery v1.9.1 but less than v4.0.0")}};g.jQueryDetection(),D();var I="alert",P="4.6.2",j="bs.alert",x="."+j,$=".data-api",K=t.default.fn[I],F="alert",Oe="fade",de="show",De="close"+x,ce="closed"+x,Dt="click"+x+$,tt='[data-dismiss="alert"]',ue=(function(){function p(o){this._element=o}var u=p.prototype;return u.close=function(e){var i=this._element;e&&(i=this._getRootElement(e));var l=this._triggerCloseEvent(i);l.isDefaultPrevented()||this._removeElement(i)},u.dispose=function(){t.default.removeData(this._element,j),this._element=null},u._getRootElement=function(e){var i=g.getSelectorFromElement(e),l=!1;return i&&(l=document.querySelector(i)),l||(l=t.default(e).closest("."+F)[0]),l},u._triggerCloseEvent=function(e){var i=t.default.Event(De);return t.default(e).trigger(i),i},u._removeElement=function(e){var i=this;if(t.default(e).removeClass(de),!t.default(e).hasClass(Oe)){this._destroyElement(e);return}var l=g.getTransitionDurationFromElement(e);t.default(e).one(g.TRANSITION_END,function(E){return i._destroyElement(e,E)}).emulateTransitionEnd(l)},u._destroyElement=function(e){t.default(e).detach().trigger(ce).remove()},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this),l=i.data(j);l||(l=new p(this),i.data(j,l)),e==="close"&&l[e](this)})},p._handleDismiss=function(e){return function(i){i&&i.preventDefault(),e.close(this)}},b(p,null,[{key:"VERSION",get:function(){return P}}]),p})();t.default(document).on(Dt,tt,ue._handleDismiss(new ue)),t.default.fn[I]=ue._jQueryInterface,t.default.fn[I].Constructor=ue,t.default.fn[I].noConflict=function(){return t.default.fn[I]=K,ue._jQueryInterface};var ke="button",En="4.6.2",at="bs.button",nt="."+at,it=".data-api",yn=t.default.fn[ke],J="active",Tn="btn",bn="focus",Sn="click"+nt+it,Cn="focus"+nt+it+" "+("blur"+nt+it),wn="load"+nt+it,ia='[data-toggle^="button"]',An='[data-toggle="buttons"]',Nn='[data-toggle="button"]',On='[data-toggle="buttons"] .btn',kt='input:not([type="hidden"])',Dn=".active",ra=".btn",Ie=(function(){function p(o){this._element=o,this.shouldAvoidTriggerChange=!1}var u=p.prototype;return u.toggle=function(){var e=!0,i=!0,l=t.default(this._element).closest(An)[0];if(l){var E=this._element.querySelector(kt);if(E){if(E.type==="radio")if(E.checked&&this._element.classList.contains(J))e=!1;else{var T=l.querySelector(Dn);T&&t.default(T).removeClass(J)}e&&((E.type==="checkbox"||E.type==="radio")&&(E.checked=!this._element.classList.contains(J)),this.shouldAvoidTriggerChange||t.default(E).trigger("change")),E.focus(),i=!1}}this._element.hasAttribute("disabled")||this._element.classList.contains("disabled")||(i&&this._element.setAttribute("aria-pressed",!this._element.classList.contains(J)),e&&t.default(this._element).toggleClass(J))},u.dispose=function(){t.default.removeData(this._element,at),this._element=null},p._jQueryInterface=function(e,i){return this.each(function(){var l=t.default(this),E=l.data(at);E||(E=new p(this),l.data(at,E)),E.shouldAvoidTriggerChange=i,e==="toggle"&&E[e]()})},b(p,null,[{key:"VERSION",get:function(){return En}}]),p})();t.default(document).on(Sn,ia,function(p){var u=p.target,o=u;if(t.default(u).hasClass(Tn)||(u=t.default(u).closest(ra)[0]),!u||u.hasAttribute("disabled")||u.classList.contains("disabled"))p.preventDefault();else{var e=u.querySelector(kt);if(e&&(e.hasAttribute("disabled")||e.classList.contains("disabled"))){p.preventDefault();return}(o.tagName==="INPUT"||u.tagName!=="LABEL")&&Ie._jQueryInterface.call(t.default(u),"toggle",o.tagName==="INPUT")}}).on(Cn,ia,function(p){var u=t.default(p.target).closest(ra)[0];t.default(u).toggleClass(bn,/^focus(in)?$/.test(p.type))}),t.default(window).on(wn,function(){for(var p=[].slice.call(document.querySelectorAll(On)),u=0,o=p.length;u<o;u++){var e=p[u],i=e.querySelector(kt);i.checked||i.hasAttribute("checked")?e.classList.add(J):e.classList.remove(J)}p=[].slice.call(document.querySelectorAll(Nn));for(var l=0,E=p.length;l<E;l++){var T=p[l];T.getAttribute("aria-pressed")==="true"?T.classList.add(J):T.classList.remove(J)}}),t.default.fn[ke]=Ie._jQueryInterface,t.default.fn[ke].Constructor=Ie,t.default.fn[ke].noConflict=function(){return t.default.fn[ke]=yn,Ie._jQueryInterface};var fe="carousel",kn="4.6.2",Me="bs.carousel",V="."+Me,sa=".data-api",In=t.default.fn[fe],Mn=37,Ln=39,Rn=500,xn=40,Pn="carousel",te="active",jn="slide",Hn="carousel-item-right",Vn="carousel-item-left",$n="carousel-item-next",Un="carousel-item-prev",Wn="pointer-event",rt="next",st="prev",Bn="left",Kn="right",Yn="slide"+V,la="slid"+V,Gn="keydown"+V,qn="mouseenter"+V,Qn="mouseleave"+V,Fn="touchstart"+V,zn="touchmove"+V,Jn="touchend"+V,Xn="pointerdown"+V,Zn="pointerup"+V,ei="dragstart"+V,ti="load"+V+sa,ai="click"+V+sa,ni=".active",lt=".active.carousel-item",ii=".carousel-item",ri=".carousel-item img",si=".carousel-item-next, .carousel-item-prev",li=".carousel-indicators",oi="[data-slide], [data-slide-to]",di='[data-ride="carousel"]',It={interval:5e3,keyboard:!0,slide:!1,pause:"hover",wrap:!0,touch:!0},ci={interval:"(number|boolean)",keyboard:"boolean",slide:"(boolean|string)",pause:"(string|boolean)",wrap:"boolean",touch:"boolean"},oa={TOUCH:"touch",PEN:"pen"},he=(function(){function p(o,e){this._items=null,this._interval=null,this._activeElement=null,this._isPaused=!1,this._isSliding=!1,this.touchTimeout=null,this.touchStartX=0,this.touchDeltaX=0,this._config=this._getConfig(e),this._element=o,this._indicatorsElement=this._element.querySelector(li),this._touchSupported="ontouchstart"in document.documentElement||navigator.maxTouchPoints>0,this._pointerEvent=!!(window.PointerEvent||window.MSPointerEvent),this._addEventListeners()}var u=p.prototype;return u.next=function(){this._isSliding||this._slide(rt)},u.nextWhenVisible=function(){var e=t.default(this._element);!document.hidden&&e.is(":visible")&&e.css("visibility")!=="hidden"&&this.next()},u.prev=function(){this._isSliding||this._slide(st)},u.pause=function(e){e||(this._isPaused=!0),this._element.querySelector(si)&&(g.triggerTransitionEnd(this._element),this.cycle(!0)),clearInterval(this._interval),this._interval=null},u.cycle=function(e){e||(this._isPaused=!1),this._interval&&(clearInterval(this._interval),this._interval=null),this._config.interval&&!this._isPaused&&(this._updateInterval(),this._interval=setInterval((document.visibilityState?this.nextWhenVisible:this.next).bind(this),this._config.interval))},u.to=function(e){var i=this;this._activeElement=this._element.querySelector(lt);var l=this._getItemIndex(this._activeElement);if(!(e>this._items.length-1||e<0)){if(this._isSliding){t.default(this._element).one(la,function(){return i.to(e)});return}if(l===e){this.pause(),this.cycle();return}var E=e>l?rt:st;this._slide(E,this._items[e])}},u.dispose=function(){t.default(this._element).off(V),t.default.removeData(this._element,Me),this._items=null,this._config=null,this._element=null,this._interval=null,this._isPaused=null,this._isSliding=null,this._activeElement=null,this._indicatorsElement=null},u._getConfig=function(e){return e=y({},It,e),g.typeCheckConfig(fe,e,ci),e},u._handleSwipe=function(){var e=Math.abs(this.touchDeltaX);if(!(e<=xn)){var i=e/this.touchDeltaX;this.touchDeltaX=0,i>0&&this.prev(),i<0&&this.next()}},u._addEventListeners=function(){var e=this;this._config.keyboard&&t.default(this._element).on(Gn,function(i){return e._keydown(i)}),this._config.pause==="hover"&&t.default(this._element).on(qn,function(i){return e.pause(i)}).on(Qn,function(i){return e.cycle(i)}),this._config.touch&&this._addTouchEventListeners()},u._addTouchEventListeners=function(){var e=this;if(this._touchSupported){var i=function(C){e._pointerEvent&&oa[C.originalEvent.pointerType.toUpperCase()]?e.touchStartX=C.originalEvent.clientX:e._pointerEvent||(e.touchStartX=C.originalEvent.touches[0].clientX)},l=function(C){e.touchDeltaX=C.originalEvent.touches&&C.originalEvent.touches.length>1?0:C.originalEvent.touches[0].clientX-e.touchStartX},E=function(C){e._pointerEvent&&oa[C.originalEvent.pointerType.toUpperCase()]&&(e.touchDeltaX=C.originalEvent.clientX-e.touchStartX),e._handleSwipe(),e._config.pause==="hover"&&(e.pause(),e.touchTimeout&&clearTimeout(e.touchTimeout),e.touchTimeout=setTimeout(function(N){return e.cycle(N)},Rn+e._config.interval))};t.default(this._element.querySelectorAll(ri)).on(ei,function(T){return T.preventDefault()}),this._pointerEvent?(t.default(this._element).on(Xn,function(T){return i(T)}),t.default(this._element).on(Zn,function(T){return E(T)}),this._element.classList.add(Wn)):(t.default(this._element).on(Fn,function(T){return i(T)}),t.default(this._element).on(zn,function(T){return l(T)}),t.default(this._element).on(Jn,function(T){return E(T)}))}},u._keydown=function(e){if(!/input|textarea/i.test(e.target.tagName))switch(e.which){case Mn:e.preventDefault(),this.prev();break;case Ln:e.preventDefault(),this.next();break}},u._getItemIndex=function(e){return this._items=e&&e.parentNode?[].slice.call(e.parentNode.querySelectorAll(ii)):[],this._items.indexOf(e)},u._getItemByDirection=function(e,i){var l=e===rt,E=e===st,T=this._getItemIndex(i),C=this._items.length-1,N=E&&T===0||l&&T===C;if(N&&!this._config.wrap)return i;var M=e===st?-1:1,R=(T+M)%this._items.length;return R===-1?this._items[this._items.length-1]:this._items[R]},u._triggerSlideEvent=function(e,i){var l=this._getItemIndex(e),E=this._getItemIndex(this._element.querySelector(lt)),T=t.default.Event(Yn,{relatedTarget:e,direction:i,from:E,to:l});return t.default(this._element).trigger(T),T},u._setActiveIndicatorElement=function(e){if(this._indicatorsElement){var i=[].slice.call(this._indicatorsElement.querySelectorAll(ni));t.default(i).removeClass(te);var l=this._indicatorsElement.children[this._getItemIndex(e)];l&&t.default(l).addClass(te)}},u._updateInterval=function(){var e=this._activeElement||this._element.querySelector(lt);if(e){var i=parseInt(e.getAttribute("data-interval"),10);i?(this._config.defaultInterval=this._config.defaultInterval||this._config.interval,this._config.interval=i):this._config.interval=this._config.defaultInterval||this._config.interval}},u._slide=function(e,i){var l=this,E=this._element.querySelector(lt),T=this._getItemIndex(E),C=i||E&&this._getItemByDirection(e,E),N=this._getItemIndex(C),M=!!this._interval,R,H,z;if(e===rt?(R=Vn,H=$n,z=Bn):(R=Hn,H=Un,z=Kn),C&&t.default(C).hasClass(te)){this._isSliding=!1;return}var Q=this._triggerSlideEvent(C,z);if(!Q.isDefaultPrevented()&&!(!E||!C)){this._isSliding=!0,M&&this.pause(),this._setActiveIndicatorElement(C),this._activeElement=C;var ye=t.default.Event(la,{relatedTarget:C,direction:z,from:T,to:N});if(t.default(this._element).hasClass(jn)){t.default(C).addClass(H),g.reflow(C),t.default(E).addClass(R),t.default(C).addClass(R);var Kt=g.getTransitionDurationFromElement(E);t.default(E).one(g.TRANSITION_END,function(){t.default(C).removeClass(R+" "+H).addClass(te),t.default(E).removeClass(te+" "+H+" "+R),l._isSliding=!1,setTimeout(function(){return t.default(l._element).trigger(ye)},0)}).emulateTransitionEnd(Kt)}else t.default(E).removeClass(te),t.default(C).addClass(te),this._isSliding=!1,t.default(this._element).trigger(ye);M&&this.cycle()}},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this).data(Me),l=y({},It,t.default(this).data());typeof e=="object"&&(l=y({},l,e));var E=typeof e=="string"?e:l.slide;if(i||(i=new p(this,l),t.default(this).data(Me,i)),typeof e=="number")i.to(e);else if(typeof E=="string"){if(typeof i[E]>"u")throw new TypeError('No method named "'+E+'"');i[E]()}else l.interval&&l.ride&&(i.pause(),i.cycle())})},p._dataApiClickHandler=function(e){var i=g.getSelectorFromElement(this);if(i){var l=t.default(i)[0];if(!(!l||!t.default(l).hasClass(Pn))){var E=y({},t.default(l).data(),t.default(this).data()),T=this.getAttribute("data-slide-to");T&&(E.interval=!1),p._jQueryInterface.call(t.default(l),E),T&&t.default(l).data(Me).to(T),e.preventDefault()}}},b(p,null,[{key:"VERSION",get:function(){return kn}},{key:"Default",get:function(){return It}}]),p})();t.default(document).on(ai,oi,he._dataApiClickHandler),t.default(window).on(ti,function(){for(var p=[].slice.call(document.querySelectorAll(di)),u=0,o=p.length;u<o;u++){var e=t.default(p[u]);he._jQueryInterface.call(e,e.data())}}),t.default.fn[fe]=he._jQueryInterface,t.default.fn[fe].Constructor=he,t.default.fn[fe].noConflict=function(){return t.default.fn[fe]=In,he._jQueryInterface};var me="collapse",ui="4.6.2",ae="bs.collapse",Le="."+ae,fi=".data-api",hi=t.default.fn[me],ne="show",Re="collapse",ot="collapsing",Mt="collapsed",da="width",mi="height",vi="show"+Le,pi="shown"+Le,gi="hide"+Le,_i="hidden"+Le,Ei="click"+Le+fi,yi=".show, .collapsing",ca='[data-toggle="collapse"]',Lt={toggle:!0,parent:""},Ti={toggle:"boolean",parent:"(string|element)"},xe=(function(){function p(o,e){this._isTransitioning=!1,this._element=o,this._config=this._getConfig(e),this._triggerArray=[].slice.call(document.querySelectorAll('[data-toggle="collapse"][href="#'+o.id+'"],'+('[data-toggle="collapse"][data-target="#'+o.id+'"]')));for(var i=[].slice.call(document.querySelectorAll(ca)),l=0,E=i.length;l<E;l++){var T=i[l],C=g.getSelectorFromElement(T),N=[].slice.call(document.querySelectorAll(C)).filter(function(M){return M===o});C!==null&&N.length>0&&(this._selector=C,this._triggerArray.push(T))}this._parent=this._config.parent?this._getParent():null,this._config.parent||this._addAriaAndCollapsedClass(this._element,this._triggerArray),this._config.toggle&&this.toggle()}var u=p.prototype;return u.toggle=function(){t.default(this._element).hasClass(ne)?this.hide():this.show()},u.show=function(){var e=this;if(!(this._isTransitioning||t.default(this._element).hasClass(ne))){var i,l;if(this._parent&&(i=[].slice.call(this._parent.querySelectorAll(yi)).filter(function(H){return typeof e._config.parent=="string"?H.getAttribute("data-parent")===e._config.parent:H.classList.contains(Re)}),i.length===0&&(i=null)),!(i&&(l=t.default(i).not(this._selector).data(ae),l&&l._isTransitioning))){var E=t.default.Event(vi);if(t.default(this._element).trigger(E),!E.isDefaultPrevented()){i&&(p._jQueryInterface.call(t.default(i).not(this._selector),"hide"),l||t.default(i).data(ae,null));var T=this._getDimension();t.default(this._element).removeClass(Re).addClass(ot),this._element.style[T]=0,this._triggerArray.length&&t.default(this._triggerArray).removeClass(Mt).attr("aria-expanded",!0),this.setTransitioning(!0);var C=function(){t.default(e._element).removeClass(ot).addClass(Re+" "+ne),e._element.style[T]="",e.setTransitioning(!1),t.default(e._element).trigger(pi)},N=T[0].toUpperCase()+T.slice(1),M="scroll"+N,R=g.getTransitionDurationFromElement(this._element);t.default(this._element).one(g.TRANSITION_END,C).emulateTransitionEnd(R),this._element.style[T]=this._element[M]+"px"}}}},u.hide=function(){var e=this;if(!(this._isTransitioning||!t.default(this._element).hasClass(ne))){var i=t.default.Event(gi);if(t.default(this._element).trigger(i),!i.isDefaultPrevented()){var l=this._getDimension();this._element.style[l]=this._element.getBoundingClientRect()[l]+"px",g.reflow(this._element),t.default(this._element).addClass(ot).removeClass(Re+" "+ne);var E=this._triggerArray.length;if(E>0)for(var T=0;T<E;T++){var C=this._triggerArray[T],N=g.getSelectorFromElement(C);if(N!==null){var M=t.default([].slice.call(document.querySelectorAll(N)));M.hasClass(ne)||t.default(C).addClass(Mt).attr("aria-expanded",!1)}}this.setTransitioning(!0);var R=function(){e.setTransitioning(!1),t.default(e._element).removeClass(ot).addClass(Re).trigger(_i)};this._element.style[l]="";var H=g.getTransitionDurationFromElement(this._element);t.default(this._element).one(g.TRANSITION_END,R).emulateTransitionEnd(H)}}},u.setTransitioning=function(e){this._isTransitioning=e},u.dispose=function(){t.default.removeData(this._element,ae),this._config=null,this._parent=null,this._element=null,this._triggerArray=null,this._isTransitioning=null},u._getConfig=function(e){return e=y({},Lt,e),e.toggle=!!e.toggle,g.typeCheckConfig(me,e,Ti),e},u._getDimension=function(){var e=t.default(this._element).hasClass(da);return e?da:mi},u._getParent=function(){var e=this,i;g.isElement(this._config.parent)?(i=this._config.parent,typeof this._config.parent.jquery<"u"&&(i=this._config.parent[0])):i=document.querySelector(this._config.parent);var l='[data-toggle="collapse"][data-parent="'+this._config.parent+'"]',E=[].slice.call(i.querySelectorAll(l));return t.default(E).each(function(T,C){e._addAriaAndCollapsedClass(p._getTargetFromElement(C),[C])}),i},u._addAriaAndCollapsedClass=function(e,i){var l=t.default(e).hasClass(ne);i.length&&t.default(i).toggleClass(Mt,!l).attr("aria-expanded",l)},p._getTargetFromElement=function(e){var i=g.getSelectorFromElement(e);return i?document.querySelector(i):null},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this),l=i.data(ae),E=y({},Lt,i.data(),typeof e=="object"&&e?e:{});if(!l&&E.toggle&&typeof e=="string"&&/show|hide/.test(e)&&(E.toggle=!1),l||(l=new p(this,E),i.data(ae,l)),typeof e=="string"){if(typeof l[e]>"u")throw new TypeError('No method named "'+e+'"');l[e]()}})},b(p,null,[{key:"VERSION",get:function(){return ui}},{key:"Default",get:function(){return Lt}}]),p})();t.default(document).on(Ei,ca,function(p){p.currentTarget.tagName==="A"&&p.preventDefault();var u=t.default(this),o=g.getSelectorFromElement(this),e=[].slice.call(document.querySelectorAll(o));t.default(e).each(function(){var i=t.default(this),l=i.data(ae),E=l?"toggle":u.data();xe._jQueryInterface.call(i,E)})}),t.default.fn[me]=xe._jQueryInterface,t.default.fn[me].Constructor=xe,t.default.fn[me].noConflict=function(){return t.default.fn[me]=hi,xe._jQueryInterface};var ve="dropdown",bi="4.6.2",Pe="bs.dropdown",X="."+Pe,Rt=".data-api",Si=t.default.fn[ve],je=27,ua=32,fa=9,xt=38,Pt=40,Ci=3,wi=new RegExp(xt+"|"+Pt+"|"+je),dt="disabled",Y="show",Ai="dropup",Ni="dropright",Oi="dropleft",ha="dropdown-menu-right",Di="position-static",ma="hide"+X,va="hidden"+X,ki="show"+X,Ii="shown"+X,Mi="click"+X,jt="click"+X+Rt,pa="keydown"+X+Rt,Li="keyup"+X+Rt,ct='[data-toggle="dropdown"]',Ri=".dropdown form",Ht=".dropdown-menu",xi=".navbar-nav",Pi=".dropdown-menu .dropdown-item:not(.disabled):not(:disabled)",ji="top-start",Hi="top-end",Vi="bottom-start",$i="bottom-end",Ui="right-start",Wi="left-start",Bi={offset:0,flip:!0,boundary:"scrollParent",reference:"toggle",display:"dynamic",popperConfig:null},Ki={offset:"(number|string|function)",flip:"boolean",boundary:"(string|element)",reference:"(string|element)",display:"string",popperConfig:"(null|object)"},Z=(function(){function p(o,e){this._element=o,this._popper=null,this._config=this._getConfig(e),this._menu=this._getMenuElement(),this._inNavbar=this._detectNavbar(),this._addEventListeners()}var u=p.prototype;return u.toggle=function(){if(!(this._element.disabled||t.default(this._element).hasClass(dt))){var e=t.default(this._menu).hasClass(Y);p._clearMenus(),!e&&this.show(!0)}},u.show=function(e){if(e===void 0&&(e=!1),!(this._element.disabled||t.default(this._element).hasClass(dt)||t.default(this._menu).hasClass(Y))){var i={relatedTarget:this._element},l=t.default.Event(ki,i),E=p._getParentFromElement(this._element);if(t.default(E).trigger(l),!l.isDefaultPrevented()){if(!this._inNavbar&&e){if(typeof r.default>"u")throw new TypeError("Bootstrap's dropdowns require Popper (https://popper.js.org)");var T=this._element;this._config.reference==="parent"?T=E:g.isElement(this._config.reference)&&(T=this._config.reference,typeof this._config.reference.jquery<"u"&&(T=this._config.reference[0])),this._config.boundary!=="scrollParent"&&t.default(E).addClass(Di),this._popper=new r.default(T,this._menu,this._getPopperConfig())}"ontouchstart"in document.documentElement&&t.default(E).closest(xi).length===0&&t.default(document.body).children().on("mouseover",null,t.default.noop),this._element.focus(),this._element.setAttribute("aria-expanded",!0),t.default(this._menu).toggleClass(Y),t.default(E).toggleClass(Y).trigger(t.default.Event(Ii,i))}}},u.hide=function(){if(!(this._element.disabled||t.default(this._element).hasClass(dt)||!t.default(this._menu).hasClass(Y))){var e={relatedTarget:this._element},i=t.default.Event(ma,e),l=p._getParentFromElement(this._element);t.default(l).trigger(i),!i.isDefaultPrevented()&&(this._popper&&this._popper.destroy(),t.default(this._menu).toggleClass(Y),t.default(l).toggleClass(Y).trigger(t.default.Event(va,e)))}},u.dispose=function(){t.default.removeData(this._element,Pe),t.default(this._element).off(X),this._element=null,this._menu=null,this._popper!==null&&(this._popper.destroy(),this._popper=null)},u.update=function(){this._inNavbar=this._detectNavbar(),this._popper!==null&&this._popper.scheduleUpdate()},u._addEventListeners=function(){var e=this;t.default(this._element).on(Mi,function(i){i.preventDefault(),i.stopPropagation(),e.toggle()})},u._getConfig=function(e){return e=y({},this.constructor.Default,t.default(this._element).data(),e),g.typeCheckConfig(ve,e,this.constructor.DefaultType),e},u._getMenuElement=function(){if(!this._menu){var e=p._getParentFromElement(this._element);e&&(this._menu=e.querySelector(Ht))}return this._menu},u._getPlacement=function(){var e=t.default(this._element.parentNode),i=Vi;return e.hasClass(Ai)?i=t.default(this._menu).hasClass(ha)?Hi:ji:e.hasClass(Ni)?i=Ui:e.hasClass(Oi)?i=Wi:t.default(this._menu).hasClass(ha)&&(i=$i),i},u._detectNavbar=function(){return t.default(this._element).closest(".navbar").length>0},u._getOffset=function(){var e=this,i={};return typeof this._config.offset=="function"?i.fn=function(l){return l.offsets=y({},l.offsets,e._config.offset(l.offsets,e._element)),l}:i.offset=this._config.offset,i},u._getPopperConfig=function(){var e={placement:this._getPlacement(),modifiers:{offset:this._getOffset(),flip:{enabled:this._config.flip},preventOverflow:{boundariesElement:this._config.boundary}}};return this._config.display==="static"&&(e.modifiers.applyStyle={enabled:!1}),y({},e,this._config.popperConfig)},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this).data(Pe),l=typeof e=="object"?e:null;if(i||(i=new p(this,l),t.default(this).data(Pe,i)),typeof e=="string"){if(typeof i[e]>"u")throw new TypeError('No method named "'+e+'"');i[e]()}})},p._clearMenus=function(e){if(!(e&&(e.which===Ci||e.type==="keyup"&&e.which!==fa)))for(var i=[].slice.call(document.querySelectorAll(ct)),l=0,E=i.length;l<E;l++){var T=p._getParentFromElement(i[l]),C=t.default(i[l]).data(Pe),N={relatedTarget:i[l]};if(e&&e.type==="click"&&(N.clickEvent=e),!!C){var M=C._menu;if(t.default(T).hasClass(Y)&&!(e&&(e.type==="click"&&/input|textarea/i.test(e.target.tagName)||e.type==="keyup"&&e.which===fa)&&t.default.contains(T,e.target))){var R=t.default.Event(ma,N);t.default(T).trigger(R),!R.isDefaultPrevented()&&("ontouchstart"in document.documentElement&&t.default(document.body).children().off("mouseover",null,t.default.noop),i[l].setAttribute("aria-expanded","false"),C._popper&&C._popper.destroy(),t.default(M).removeClass(Y),t.default(T).removeClass(Y).trigger(t.default.Event(va,N)))}}}},p._getParentFromElement=function(e){var i,l=g.getSelectorFromElement(e);return l&&(i=document.querySelector(l)),i||e.parentNode},p._dataApiKeydownHandler=function(e){if(!(/input|textarea/i.test(e.target.tagName)?e.which===ua||e.which!==je&&(e.which!==Pt&&e.which!==xt||t.default(e.target).closest(Ht).length):!wi.test(e.which))&&!(this.disabled||t.default(this).hasClass(dt))){var i=p._getParentFromElement(this),l=t.default(i).hasClass(Y);if(!(!l&&e.which===je)){if(e.preventDefault(),e.stopPropagation(),!l||e.which===je||e.which===ua){e.which===je&&t.default(i.querySelector(ct)).trigger("focus"),t.default(this).trigger("click");return}var E=[].slice.call(i.querySelectorAll(Pi)).filter(function(C){return t.default(C).is(":visible")});if(E.length!==0){var T=E.indexOf(e.target);e.which===xt&&T>0&&T--,e.which===Pt&&T<E.length-1&&T++,T<0&&(T=0),E[T].focus()}}}},b(p,null,[{key:"VERSION",get:function(){return bi}},{key:"Default",get:function(){return Bi}},{key:"DefaultType",get:function(){return Ki}}]),p})();t.default(document).on(pa,ct,Z._dataApiKeydownHandler).on(pa,Ht,Z._dataApiKeydownHandler).on(jt+" "+Li,Z._clearMenus).on(jt,ct,function(p){p.preventDefault(),p.stopPropagation(),Z._jQueryInterface.call(t.default(this),"toggle")}).on(jt,Ri,function(p){p.stopPropagation()}),t.default.fn[ve]=Z._jQueryInterface,t.default.fn[ve].Constructor=Z,t.default.fn[ve].noConflict=function(){return t.default.fn[ve]=Si,Z._jQueryInterface};var pe="modal",Yi="4.6.2",He="bs.modal",U="."+He,Gi=".data-api",qi=t.default.fn[pe],ga=27,Qi="modal-dialog-scrollable",Fi="modal-scrollbar-measure",zi="modal-backdrop",_a="modal-open",ge="fade",ut="show",Ea="modal-static",Ji="hide"+U,Xi="hidePrevented"+U,ya="hidden"+U,Ta="show"+U,Zi="shown"+U,ft="focusin"+U,ba="resize"+U,Vt="click.dismiss"+U,Sa="keydown.dismiss"+U,er="mouseup.dismiss"+U,Ca="mousedown.dismiss"+U,tr="click"+U+Gi,ar=".modal-dialog",nr=".modal-body",ir='[data-toggle="modal"]',rr='[data-dismiss="modal"]',wa=".fixed-top, .fixed-bottom, .is-fixed, .sticky-top",Aa=".sticky-top",$t={backdrop:!0,keyboard:!0,focus:!0,show:!0},sr={backdrop:"(boolean|string)",keyboard:"boolean",focus:"boolean",show:"boolean"},Ve=(function(){function p(o,e){this._config=this._getConfig(e),this._element=o,this._dialog=o.querySelector(ar),this._backdrop=null,this._isShown=!1,this._isBodyOverflowing=!1,this._ignoreBackdropClick=!1,this._isTransitioning=!1,this._scrollbarWidth=0}var u=p.prototype;return u.toggle=function(e){return this._isShown?this.hide():this.show(e)},u.show=function(e){var i=this;if(!(this._isShown||this._isTransitioning)){var l=t.default.Event(Ta,{relatedTarget:e});t.default(this._element).trigger(l),!l.isDefaultPrevented()&&(this._isShown=!0,t.default(this._element).hasClass(ge)&&(this._isTransitioning=!0),this._checkScrollbar(),this._setScrollbar(),this._adjustDialog(),this._setEscapeEvent(),this._setResizeEvent(),t.default(this._element).on(Vt,rr,function(E){return i.hide(E)}),t.default(this._dialog).on(Ca,function(){t.default(i._element).one(er,function(E){t.default(E.target).is(i._element)&&(i._ignoreBackdropClick=!0)})}),this._showBackdrop(function(){return i._showElement(e)}))}},u.hide=function(e){var i=this;if(e&&e.preventDefault(),!(!this._isShown||this._isTransitioning)){var l=t.default.Event(Ji);if(t.default(this._element).trigger(l),!(!this._isShown||l.isDefaultPrevented())){this._isShown=!1;var E=t.default(this._element).hasClass(ge);if(E&&(this._isTransitioning=!0),this._setEscapeEvent(),this._setResizeEvent(),t.default(document).off(ft),t.default(this._element).removeClass(ut),t.default(this._element).off(Vt),t.default(this._dialog).off(Ca),E){var T=g.getTransitionDurationFromElement(this._element);t.default(this._element).one(g.TRANSITION_END,function(C){return i._hideModal(C)}).emulateTransitionEnd(T)}else this._hideModal()}}},u.dispose=function(){[window,this._element,this._dialog].forEach(function(e){return t.default(e).off(U)}),t.default(document).off(ft),t.default.removeData(this._element,He),this._config=null,this._element=null,this._dialog=null,this._backdrop=null,this._isShown=null,this._isBodyOverflowing=null,this._ignoreBackdropClick=null,this._isTransitioning=null,this._scrollbarWidth=null},u.handleUpdate=function(){this._adjustDialog()},u._getConfig=function(e){return e=y({},$t,e),g.typeCheckConfig(pe,e,sr),e},u._triggerBackdropTransition=function(){var e=this,i=t.default.Event(Xi);if(t.default(this._element).trigger(i),!i.isDefaultPrevented()){var l=this._element.scrollHeight>document.documentElement.clientHeight;l||(this._element.style.overflowY="hidden"),this._element.classList.add(Ea);var E=g.getTransitionDurationFromElement(this._dialog);t.default(this._element).off(g.TRANSITION_END),t.default(this._element).one(g.TRANSITION_END,function(){e._element.classList.remove(Ea),l||t.default(e._element).one(g.TRANSITION_END,function(){e._element.style.overflowY=""}).emulateTransitionEnd(e._element,E)}).emulateTransitionEnd(E),this._element.focus()}},u._showElement=function(e){var i=this,l=t.default(this._element).hasClass(ge),E=this._dialog?this._dialog.querySelector(nr):null;(!this._element.parentNode||this._element.parentNode.nodeType!==Node.ELEMENT_NODE)&&document.body.appendChild(this._element),this._element.style.display="block",this._element.removeAttribute("aria-hidden"),this._element.setAttribute("aria-modal",!0),this._element.setAttribute("role","dialog"),t.default(this._dialog).hasClass(Qi)&&E?E.scrollTop=0:this._element.scrollTop=0,l&&g.reflow(this._element),t.default(this._element).addClass(ut),this._config.focus&&this._enforceFocus();var T=t.default.Event(Zi,{relatedTarget:e}),C=function(){i._config.focus&&i._element.focus(),i._isTransitioning=!1,t.default(i._element).trigger(T)};if(l){var N=g.getTransitionDurationFromElement(this._dialog);t.default(this._dialog).one(g.TRANSITION_END,C).emulateTransitionEnd(N)}else C()},u._enforceFocus=function(){var e=this;t.default(document).off(ft).on(ft,function(i){document!==i.target&&e._element!==i.target&&t.default(e._element).has(i.target).length===0&&e._element.focus()})},u._setEscapeEvent=function(){var e=this;this._isShown?t.default(this._element).on(Sa,function(i){e._config.keyboard&&i.which===ga?(i.preventDefault(),e.hide()):!e._config.keyboard&&i.which===ga&&e._triggerBackdropTransition()}):this._isShown||t.default(this._element).off(Sa)},u._setResizeEvent=function(){var e=this;this._isShown?t.default(window).on(ba,function(i){return e.handleUpdate(i)}):t.default(window).off(ba)},u._hideModal=function(){var e=this;this._element.style.display="none",this._element.setAttribute("aria-hidden",!0),this._element.removeAttribute("aria-modal"),this._element.removeAttribute("role"),this._isTransitioning=!1,this._showBackdrop(function(){t.default(document.body).removeClass(_a),e._resetAdjustments(),e._resetScrollbar(),t.default(e._element).trigger(ya)})},u._removeBackdrop=function(){this._backdrop&&(t.default(this._backdrop).remove(),this._backdrop=null)},u._showBackdrop=function(e){var i=this,l=t.default(this._element).hasClass(ge)?ge:"";if(this._isShown&&this._config.backdrop){if(this._backdrop=document.createElement("div"),this._backdrop.className=zi,l&&this._backdrop.classList.add(l),t.default(this._backdrop).appendTo(document.body),t.default(this._element).on(Vt,function(N){if(i._ignoreBackdropClick){i._ignoreBackdropClick=!1;return}N.target===N.currentTarget&&(i._config.backdrop==="static"?i._triggerBackdropTransition():i.hide())}),l&&g.reflow(this._backdrop),t.default(this._backdrop).addClass(ut),!e)return;if(!l){e();return}var E=g.getTransitionDurationFromElement(this._backdrop);t.default(this._backdrop).one(g.TRANSITION_END,e).emulateTransitionEnd(E)}else if(!this._isShown&&this._backdrop){t.default(this._backdrop).removeClass(ut);var T=function(){i._removeBackdrop(),e&&e()};if(t.default(this._element).hasClass(ge)){var C=g.getTransitionDurationFromElement(this._backdrop);t.default(this._backdrop).one(g.TRANSITION_END,T).emulateTransitionEnd(C)}else T()}else e&&e()},u._adjustDialog=function(){var e=this._element.scrollHeight>document.documentElement.clientHeight;!this._isBodyOverflowing&&e&&(this._element.style.paddingLeft=this._scrollbarWidth+"px"),this._isBodyOverflowing&&!e&&(this._element.style.paddingRight=this._scrollbarWidth+"px")},u._resetAdjustments=function(){this._element.style.paddingLeft="",this._element.style.paddingRight=""},u._checkScrollbar=function(){var e=document.body.getBoundingClientRect();this._isBodyOverflowing=Math.round(e.left+e.right)<window.innerWidth,this._scrollbarWidth=this._getScrollbarWidth()},u._setScrollbar=function(){var e=this;if(this._isBodyOverflowing){var i=[].slice.call(document.querySelectorAll(wa)),l=[].slice.call(document.querySelectorAll(Aa));t.default(i).each(function(C,N){var M=N.style.paddingRight,R=t.default(N).css("padding-right");t.default(N).data("padding-right",M).css("padding-right",parseFloat(R)+e._scrollbarWidth+"px")}),t.default(l).each(function(C,N){var M=N.style.marginRight,R=t.default(N).css("margin-right");t.default(N).data("margin-right",M).css("margin-right",parseFloat(R)-e._scrollbarWidth+"px")});var E=document.body.style.paddingRight,T=t.default(document.body).css("padding-right");t.default(document.body).data("padding-right",E).css("padding-right",parseFloat(T)+this._scrollbarWidth+"px")}t.default(document.body).addClass(_a)},u._resetScrollbar=function(){var e=[].slice.call(document.querySelectorAll(wa));t.default(e).each(function(E,T){var C=t.default(T).data("padding-right");t.default(T).removeData("padding-right"),T.style.paddingRight=C||""});var i=[].slice.call(document.querySelectorAll(""+Aa));t.default(i).each(function(E,T){var C=t.default(T).data("margin-right");typeof C<"u"&&t.default(T).css("margin-right",C).removeData("margin-right")});var l=t.default(document.body).data("padding-right");t.default(document.body).removeData("padding-right"),document.body.style.paddingRight=l||""},u._getScrollbarWidth=function(){var e=document.createElement("div");e.className=Fi,document.body.appendChild(e);var i=e.getBoundingClientRect().width-e.clientWidth;return document.body.removeChild(e),i},p._jQueryInterface=function(e,i){return this.each(function(){var l=t.default(this).data(He),E=y({},$t,t.default(this).data(),typeof e=="object"&&e?e:{});if(l||(l=new p(this,E),t.default(this).data(He,l)),typeof e=="string"){if(typeof l[e]>"u")throw new TypeError('No method named "'+e+'"');l[e](i)}else E.show&&l.show(i)})},b(p,null,[{key:"VERSION",get:function(){return Yi}},{key:"Default",get:function(){return $t}}]),p})();t.default(document).on(tr,ir,function(p){var u=this,o,e=g.getSelectorFromElement(this);e&&(o=document.querySelector(e));var i=t.default(o).data(He)?"toggle":y({},t.default(o).data(),t.default(this).data());(this.tagName==="A"||this.tagName==="AREA")&&p.preventDefault();var l=t.default(o).one(Ta,function(E){E.isDefaultPrevented()||l.one(ya,function(){t.default(u).is(":visible")&&u.focus()})});Ve._jQueryInterface.call(t.default(o),i,this)}),t.default.fn[pe]=Ve._jQueryInterface,t.default.fn[pe].Constructor=Ve,t.default.fn[pe].noConflict=function(){return t.default.fn[pe]=qi,Ve._jQueryInterface};var lr=["background","cite","href","itemtype","longdesc","poster","src","xlink:href"],or=/^aria-[\w-]*$/i,dr={"*":["class","dir","id","lang","role",or],a:["target","href","title","rel"],area:[],b:[],br:[],col:[],code:[],div:[],em:[],hr:[],h1:[],h2:[],h3:[],h4:[],h5:[],h6:[],i:[],img:["src","srcset","alt","title","width","height"],li:[],ol:[],p:[],pre:[],s:[],small:[],span:[],sub:[],sup:[],strong:[],u:[],ul:[]},cr=/^(?:(?:https?|mailto|ftp|tel|file|sms):|[^#&/:?]*(?:[#/?]|$))/i,ur=/^data:(?:image\/(?:bmp|gif|jpeg|jpg|png|tiff|webp)|video\/(?:mpeg|mp4|ogg|webm)|audio\/(?:mp3|oga|ogg|opus));base64,[\d+/a-z]+=*$/i;function fr(p,u){var o=p.nodeName.toLowerCase();if(u.indexOf(o)!==-1)return lr.indexOf(o)!==-1?!!(cr.test(p.nodeValue)||ur.test(p.nodeValue)):!0;for(var e=u.filter(function(E){return E instanceof RegExp}),i=0,l=e.length;i<l;i++)if(e[i].test(o))return!0;return!1}function Na(p,u,o){if(p.length===0)return p;if(o&&typeof o=="function")return o(p);for(var e=new window.DOMParser,i=e.parseFromString(p,"text/html"),l=Object.keys(u),E=[].slice.call(i.body.querySelectorAll("*")),T=function(H,z){var Q=E[H],ye=Q.nodeName.toLowerCase();if(l.indexOf(Q.nodeName.toLowerCase())===-1)return Q.parentNode.removeChild(Q),"continue";var Kt=[].slice.call(Q.attributes),Es=[].concat(u["*"]||[],u[ye]||[]);Kt.forEach(function(Wa){fr(Wa,Es)||Q.removeAttribute(Wa.nodeName)})},C=0,N=E.length;C<N;C++)var M=T(C);return i.body.innerHTML}var ie="tooltip",hr="4.6.2",ht="bs.tooltip",G="."+ht,mr=t.default.fn[ie],Oa="bs-tooltip",vr=new RegExp("(^|\\s)"+Oa+"\\S+","g"),pr=["sanitize","whiteList","sanitizeFn"],$e="fade",Ue="show",We="show",Ut="out",gr=".tooltip-inner",_r=".arrow",Be="hover",Wt="focus",Er="click",yr="manual",Tr={AUTO:"auto",TOP:"top",RIGHT:"right",BOTTOM:"bottom",LEFT:"left"},br={animation:!0,template:'<div class="tooltip" role="tooltip"><div class="arrow"></div><div class="tooltip-inner"></div></div>',trigger:"hover focus",title:"",delay:0,html:!1,selector:!1,placement:"top",offset:0,container:!1,fallbackPlacement:"flip",boundary:"scrollParent",customClass:"",sanitize:!0,sanitizeFn:null,whiteList:dr,popperConfig:null},Sr={animation:"boolean",template:"string",title:"(string|element|function)",trigger:"string",delay:"(number|object)",html:"boolean",selector:"(string|boolean)",placement:"(string|function)",offset:"(number|string|function)",container:"(string|element|boolean)",fallbackPlacement:"(string|array)",boundary:"(string|element)",customClass:"(string|function)",sanitize:"boolean",sanitizeFn:"(null|function)",whiteList:"object",popperConfig:"(null|object)"},Cr={HIDE:"hide"+G,HIDDEN:"hidden"+G,SHOW:"show"+G,SHOWN:"shown"+G,INSERTED:"inserted"+G,CLICK:"click"+G,FOCUSIN:"focusin"+G,FOCUSOUT:"focusout"+G,MOUSEENTER:"mouseenter"+G,MOUSELEAVE:"mouseleave"+G},re=(function(){function p(o,e){if(typeof r.default>"u")throw new TypeError("Bootstrap's tooltips require Popper (https://popper.js.org)");this._isEnabled=!0,this._timeout=0,this._hoverState="",this._activeTrigger={},this._popper=null,this.element=o,this.config=this._getConfig(e),this.tip=null,this._setListeners()}var u=p.prototype;return u.enable=function(){this._isEnabled=!0},u.disable=function(){this._isEnabled=!1},u.toggleEnabled=function(){this._isEnabled=!this._isEnabled},u.toggle=function(e){if(this._isEnabled)if(e){var i=this.constructor.DATA_KEY,l=t.default(e.currentTarget).data(i);l||(l=new this.constructor(e.currentTarget,this._getDelegateConfig()),t.default(e.currentTarget).data(i,l)),l._activeTrigger.click=!l._activeTrigger.click,l._isWithActiveTrigger()?l._enter(null,l):l._leave(null,l)}else{if(t.default(this.getTipElement()).hasClass(Ue)){this._leave(null,this);return}this._enter(null,this)}},u.dispose=function(){clearTimeout(this._timeout),t.default.removeData(this.element,this.constructor.DATA_KEY),t.default(this.element).off(this.constructor.EVENT_KEY),t.default(this.element).closest(".modal").off("hide.bs.modal",this._hideModalHandler),this.tip&&t.default(this.tip).remove(),this._isEnabled=null,this._timeout=null,this._hoverState=null,this._activeTrigger=null,this._popper&&this._popper.destroy(),this._popper=null,this.element=null,this.config=null,this.tip=null},u.show=function(){var e=this;if(t.default(this.element).css("display")==="none")throw new Error("Please use show on visible elements");var i=t.default.Event(this.constructor.Event.SHOW);if(this.isWithContent()&&this._isEnabled){t.default(this.element).trigger(i);var l=g.findShadowRoot(this.element),E=t.default.contains(l!==null?l:this.element.ownerDocument.documentElement,this.element);if(i.isDefaultPrevented()||!E)return;var T=this.getTipElement(),C=g.getUID(this.constructor.NAME);T.setAttribute("id",C),this.element.setAttribute("aria-describedby",C),this.setContent(),this.config.animation&&t.default(T).addClass($e);var N=typeof this.config.placement=="function"?this.config.placement.call(this,T,this.element):this.config.placement,M=this._getAttachment(N);this.addAttachmentClass(M);var R=this._getContainer();t.default(T).data(this.constructor.DATA_KEY,this),t.default.contains(this.element.ownerDocument.documentElement,this.tip)||t.default(T).appendTo(R),t.default(this.element).trigger(this.constructor.Event.INSERTED),this._popper=new r.default(this.element,T,this._getPopperConfig(M)),t.default(T).addClass(Ue),t.default(T).addClass(this.config.customClass),"ontouchstart"in document.documentElement&&t.default(document.body).children().on("mouseover",null,t.default.noop);var H=function(){e.config.animation&&e._fixTransition();var ye=e._hoverState;e._hoverState=null,t.default(e.element).trigger(e.constructor.Event.SHOWN),ye===Ut&&e._leave(null,e)};if(t.default(this.tip).hasClass($e)){var z=g.getTransitionDurationFromElement(this.tip);t.default(this.tip).one(g.TRANSITION_END,H).emulateTransitionEnd(z)}else H()}},u.hide=function(e){var i=this,l=this.getTipElement(),E=t.default.Event(this.constructor.Event.HIDE),T=function(){i._hoverState!==We&&l.parentNode&&l.parentNode.removeChild(l),i._cleanTipClass(),i.element.removeAttribute("aria-describedby"),t.default(i.element).trigger(i.constructor.Event.HIDDEN),i._popper!==null&&i._popper.destroy(),e&&e()};if(t.default(this.element).trigger(E),!E.isDefaultPrevented()){if(t.default(l).removeClass(Ue),"ontouchstart"in document.documentElement&&t.default(document.body).children().off("mouseover",null,t.default.noop),this._activeTrigger[Er]=!1,this._activeTrigger[Wt]=!1,this._activeTrigger[Be]=!1,t.default(this.tip).hasClass($e)){var C=g.getTransitionDurationFromElement(l);t.default(l).one(g.TRANSITION_END,T).emulateTransitionEnd(C)}else T();this._hoverState=""}},u.update=function(){this._popper!==null&&this._popper.scheduleUpdate()},u.isWithContent=function(){return!!this.getTitle()},u.addAttachmentClass=function(e){t.default(this.getTipElement()).addClass(Oa+"-"+e)},u.getTipElement=function(){return this.tip=this.tip||t.default(this.config.template)[0],this.tip},u.setContent=function(){var e=this.getTipElement();this.setElementContent(t.default(e.querySelectorAll(gr)),this.getTitle()),t.default(e).removeClass($e+" "+Ue)},u.setElementContent=function(e,i){if(typeof i=="object"&&(i.nodeType||i.jquery)){this.config.html?t.default(i).parent().is(e)||e.empty().append(i):e.text(t.default(i).text());return}this.config.html?(this.config.sanitize&&(i=Na(i,this.config.whiteList,this.config.sanitizeFn)),e.html(i)):e.text(i)},u.getTitle=function(){var e=this.element.getAttribute("data-original-title");return e||(e=typeof this.config.title=="function"?this.config.title.call(this.element):this.config.title),e},u._getPopperConfig=function(e){var i=this,l={placement:e,modifiers:{offset:this._getOffset(),flip:{behavior:this.config.fallbackPlacement},arrow:{element:_r},preventOverflow:{boundariesElement:this.config.boundary}},onCreate:function(T){T.originalPlacement!==T.placement&&i._handlePopperPlacementChange(T)},onUpdate:function(T){return i._handlePopperPlacementChange(T)}};return y({},l,this.config.popperConfig)},u._getOffset=function(){var e=this,i={};return typeof this.config.offset=="function"?i.fn=function(l){return l.offsets=y({},l.offsets,e.config.offset(l.offsets,e.element)),l}:i.offset=this.config.offset,i},u._getContainer=function(){return this.config.container===!1?document.body:g.isElement(this.config.container)?t.default(this.config.container):t.default(document).find(this.config.container)},u._getAttachment=function(e){return Tr[e.toUpperCase()]},u._setListeners=function(){var e=this,i=this.config.trigger.split(" ");i.forEach(function(l){if(l==="click")t.default(e.element).on(e.constructor.Event.CLICK,e.config.selector,function(C){return e.toggle(C)});else if(l!==yr){var E=l===Be?e.constructor.Event.MOUSEENTER:e.constructor.Event.FOCUSIN,T=l===Be?e.constructor.Event.MOUSELEAVE:e.constructor.Event.FOCUSOUT;t.default(e.element).on(E,e.config.selector,function(C){return e._enter(C)}).on(T,e.config.selector,function(C){return e._leave(C)})}}),this._hideModalHandler=function(){e.element&&e.hide()},t.default(this.element).closest(".modal").on("hide.bs.modal",this._hideModalHandler),this.config.selector?this.config=y({},this.config,{trigger:"manual",selector:""}):this._fixTitle()},u._fixTitle=function(){var e=typeof this.element.getAttribute("data-original-title");(this.element.getAttribute("title")||e!=="string")&&(this.element.setAttribute("data-original-title",this.element.getAttribute("title")||""),this.element.setAttribute("title",""))},u._enter=function(e,i){var l=this.constructor.DATA_KEY;if(i=i||t.default(e.currentTarget).data(l),i||(i=new this.constructor(e.currentTarget,this._getDelegateConfig()),t.default(e.currentTarget).data(l,i)),e&&(i._activeTrigger[e.type==="focusin"?Wt:Be]=!0),t.default(i.getTipElement()).hasClass(Ue)||i._hoverState===We){i._hoverState=We;return}if(clearTimeout(i._timeout),i._hoverState=We,!i.config.delay||!i.config.delay.show){i.show();return}i._timeout=setTimeout(function(){i._hoverState===We&&i.show()},i.config.delay.show)},u._leave=function(e,i){var l=this.constructor.DATA_KEY;if(i=i||t.default(e.currentTarget).data(l),i||(i=new this.constructor(e.currentTarget,this._getDelegateConfig()),t.default(e.currentTarget).data(l,i)),e&&(i._activeTrigger[e.type==="focusout"?Wt:Be]=!1),!i._isWithActiveTrigger()){if(clearTimeout(i._timeout),i._hoverState=Ut,!i.config.delay||!i.config.delay.hide){i.hide();return}i._timeout=setTimeout(function(){i._hoverState===Ut&&i.hide()},i.config.delay.hide)}},u._isWithActiveTrigger=function(){for(var e in this._activeTrigger)if(this._activeTrigger[e])return!0;return!1},u._getConfig=function(e){var i=t.default(this.element).data();return Object.keys(i).forEach(function(l){pr.indexOf(l)!==-1&&delete i[l]}),e=y({},this.constructor.Default,i,typeof e=="object"&&e?e:{}),typeof e.delay=="number"&&(e.delay={show:e.delay,hide:e.delay}),typeof e.title=="number"&&(e.title=e.title.toString()),typeof e.content=="number"&&(e.content=e.content.toString()),g.typeCheckConfig(ie,e,this.constructor.DefaultType),e.sanitize&&(e.template=Na(e.template,e.whiteList,e.sanitizeFn)),e},u._getDelegateConfig=function(){var e={};if(this.config)for(var i in this.config)this.constructor.Default[i]!==this.config[i]&&(e[i]=this.config[i]);return e},u._cleanTipClass=function(){var e=t.default(this.getTipElement()),i=e.attr("class").match(vr);i!==null&&i.length&&e.removeClass(i.join(""))},u._handlePopperPlacementChange=function(e){this.tip=e.instance.popper,this._cleanTipClass(),this.addAttachmentClass(this._getAttachment(e.placement))},u._fixTransition=function(){var e=this.getTipElement(),i=this.config.animation;e.getAttribute("x-placement")===null&&(t.default(e).removeClass($e),this.config.animation=!1,this.hide(),this.show(),this.config.animation=i)},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this),l=i.data(ht),E=typeof e=="object"&&e;if(!(!l&&/dispose|hide/.test(e))&&(l||(l=new p(this,E),i.data(ht,l)),typeof e=="string")){if(typeof l[e]>"u")throw new TypeError('No method named "'+e+'"');l[e]()}})},b(p,null,[{key:"VERSION",get:function(){return hr}},{key:"Default",get:function(){return br}},{key:"NAME",get:function(){return ie}},{key:"DATA_KEY",get:function(){return ht}},{key:"Event",get:function(){return Cr}},{key:"EVENT_KEY",get:function(){return G}},{key:"DefaultType",get:function(){return Sr}}]),p})();t.default.fn[ie]=re._jQueryInterface,t.default.fn[ie].Constructor=re,t.default.fn[ie].noConflict=function(){return t.default.fn[ie]=mr,re._jQueryInterface};var _e="popover",wr="4.6.2",mt="bs.popover",q="."+mt,Ar=t.default.fn[_e],Da="bs-popover",Nr=new RegExp("(^|\\s)"+Da+"\\S+","g"),Or="fade",Dr="show",kr=".popover-header",Ir=".popover-body",Mr=y({},re.Default,{placement:"right",trigger:"click",content:"",template:'<div class="popover" role="tooltip"><div class="arrow"></div><h3 class="popover-header"></h3><div class="popover-body"></div></div>'}),Lr=y({},re.DefaultType,{content:"(string|element|function)"}),Rr={HIDE:"hide"+q,HIDDEN:"hidden"+q,SHOW:"show"+q,SHOWN:"shown"+q,INSERTED:"inserted"+q,CLICK:"click"+q,FOCUSIN:"focusin"+q,FOCUSOUT:"focusout"+q,MOUSEENTER:"mouseenter"+q,MOUSELEAVE:"mouseleave"+q},vt=(function(p){c(u,p);function u(){return p.apply(this,arguments)||this}var o=u.prototype;return o.isWithContent=function(){return this.getTitle()||this._getContent()},o.addAttachmentClass=function(i){t.default(this.getTipElement()).addClass(Da+"-"+i)},o.getTipElement=function(){return this.tip=this.tip||t.default(this.config.template)[0],this.tip},o.setContent=function(){var i=t.default(this.getTipElement());this.setElementContent(i.find(kr),this.getTitle());var l=this._getContent();typeof l=="function"&&(l=l.call(this.element)),this.setElementContent(i.find(Ir),l),i.removeClass(Or+" "+Dr)},o._getContent=function(){return this.element.getAttribute("data-content")||this.config.content},o._cleanTipClass=function(){var i=t.default(this.getTipElement()),l=i.attr("class").match(Nr);l!==null&&l.length>0&&i.removeClass(l.join(""))},u._jQueryInterface=function(i){return this.each(function(){var l=t.default(this).data(mt),E=typeof i=="object"?i:null;if(!(!l&&/dispose|hide/.test(i))&&(l||(l=new u(this,E),t.default(this).data(mt,l)),typeof i=="string")){if(typeof l[i]>"u")throw new TypeError('No method named "'+i+'"');l[i]()}})},b(u,null,[{key:"VERSION",get:function(){return wr}},{key:"Default",get:function(){return Mr}},{key:"NAME",get:function(){return _e}},{key:"DATA_KEY",get:function(){return mt}},{key:"Event",get:function(){return Rr}},{key:"EVENT_KEY",get:function(){return q}},{key:"DefaultType",get:function(){return Lr}}]),u})(re);t.default.fn[_e]=vt._jQueryInterface,t.default.fn[_e].Constructor=vt,t.default.fn[_e].noConflict=function(){return t.default.fn[_e]=Ar,vt._jQueryInterface};var se="scrollspy",xr="4.6.2",pt="bs.scrollspy",gt="."+pt,Pr=".data-api",jr=t.default.fn[se],Hr="dropdown-item",le="active",Vr="activate"+gt,$r="scroll"+gt,Ur="load"+gt+Pr,Wr="offset",ka="position",Br='[data-spy="scroll"]',Ia=".nav, .list-group",Bt=".nav-link",Kr=".nav-item",Ma=".list-group-item",Yr=".dropdown",Gr=".dropdown-item",qr=".dropdown-toggle",La={offset:10,method:"auto",target:""},Qr={offset:"number",method:"string",target:"(string|element)"},Ke=(function(){function p(o,e){var i=this;this._element=o,this._scrollElement=o.tagName==="BODY"?window:o,this._config=this._getConfig(e),this._selector=this._config.target+" "+Bt+","+(this._config.target+" "+Ma+",")+(this._config.target+" "+Gr),this._offsets=[],this._targets=[],this._activeTarget=null,this._scrollHeight=0,t.default(this._scrollElement).on($r,function(l){return i._process(l)}),this.refresh(),this._process()}var u=p.prototype;return u.refresh=function(){var e=this,i=this._scrollElement===this._scrollElement.window?Wr:ka,l=this._config.method==="auto"?i:this._config.method,E=l===ka?this._getScrollTop():0;this._offsets=[],this._targets=[],this._scrollHeight=this._getScrollHeight();var T=[].slice.call(document.querySelectorAll(this._selector));T.map(function(C){var N,M=g.getSelectorFromElement(C);if(M&&(N=document.querySelector(M)),N){var R=N.getBoundingClientRect();if(R.width||R.height)return[t.default(N)[l]().top+E,M]}return null}).filter(Boolean).sort(function(C,N){return C[0]-N[0]}).forEach(function(C){e._offsets.push(C[0]),e._targets.push(C[1])})},u.dispose=function(){t.default.removeData(this._element,pt),t.default(this._scrollElement).off(gt),this._element=null,this._scrollElement=null,this._config=null,this._selector=null,this._offsets=null,this._targets=null,this._activeTarget=null,this._scrollHeight=null},u._getConfig=function(e){if(e=y({},La,typeof e=="object"&&e?e:{}),typeof e.target!="string"&&g.isElement(e.target)){var i=t.default(e.target).attr("id");i||(i=g.getUID(se),t.default(e.target).attr("id",i)),e.target="#"+i}return g.typeCheckConfig(se,e,Qr),e},u._getScrollTop=function(){return this._scrollElement===window?this._scrollElement.pageYOffset:this._scrollElement.scrollTop},u._getScrollHeight=function(){return this._scrollElement.scrollHeight||Math.max(document.body.scrollHeight,document.documentElement.scrollHeight)},u._getOffsetHeight=function(){return this._scrollElement===window?window.innerHeight:this._scrollElement.getBoundingClientRect().height},u._process=function(){var e=this._getScrollTop()+this._config.offset,i=this._getScrollHeight(),l=this._config.offset+i-this._getOffsetHeight();if(this._scrollHeight!==i&&this.refresh(),e>=l){var E=this._targets[this._targets.length-1];this._activeTarget!==E&&this._activate(E);return}if(this._activeTarget&&e<this._offsets[0]&&this._offsets[0]>0){this._activeTarget=null,this._clear();return}for(var T=this._offsets.length;T--;){var C=this._activeTarget!==this._targets[T]&&e>=this._offsets[T]&&(typeof this._offsets[T+1]>"u"||e<this._offsets[T+1]);C&&this._activate(this._targets[T])}},u._activate=function(e){this._activeTarget=e,this._clear();var i=this._selector.split(",").map(function(E){return E+'[data-target="'+e+'"],'+E+'[href="'+e+'"]'}),l=t.default([].slice.call(document.querySelectorAll(i.join(","))));l.hasClass(Hr)?(l.closest(Yr).find(qr).addClass(le),l.addClass(le)):(l.addClass(le),l.parents(Ia).prev(Bt+", "+Ma).addClass(le),l.parents(Ia).prev(Kr).children(Bt).addClass(le)),t.default(this._scrollElement).trigger(Vr,{relatedTarget:e})},u._clear=function(){[].slice.call(document.querySelectorAll(this._selector)).filter(function(e){return e.classList.contains(le)}).forEach(function(e){return e.classList.remove(le)})},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this).data(pt),l=typeof e=="object"&&e;if(i||(i=new p(this,l),t.default(this).data(pt,i)),typeof e=="string"){if(typeof i[e]>"u")throw new TypeError('No method named "'+e+'"');i[e]()}})},b(p,null,[{key:"VERSION",get:function(){return xr}},{key:"Default",get:function(){return La}}]),p})();t.default(window).on(Ur,function(){for(var p=[].slice.call(document.querySelectorAll(Br)),u=p.length,o=u;o--;){var e=t.default(p[o]);Ke._jQueryInterface.call(e,e.data())}}),t.default.fn[se]=Ke._jQueryInterface,t.default.fn[se].Constructor=Ke,t.default.fn[se].noConflict=function(){return t.default.fn[se]=jr,Ke._jQueryInterface};var Ye="tab",Fr="4.6.2",_t="bs.tab",Ge="."+_t,zr=".data-api",Jr=t.default.fn[Ye],Xr="dropdown-menu",qe="active",Zr="disabled",Ra="fade",xa="show",es="hide"+Ge,ts="hidden"+Ge,as="show"+Ge,ns="shown"+Ge,is="click"+Ge+zr,rs=".dropdown",ss=".nav, .list-group",Pa=".active",ja="> li > .active",ls='[data-toggle="tab"], [data-toggle="pill"], [data-toggle="list"]',os=".dropdown-toggle",ds="> .dropdown-menu .active",Qe=(function(){function p(o){this._element=o}var u=p.prototype;return u.show=function(){var e=this;if(!(this._element.parentNode&&this._element.parentNode.nodeType===Node.ELEMENT_NODE&&t.default(this._element).hasClass(qe)||t.default(this._element).hasClass(Zr)||this._element.hasAttribute("disabled"))){var i,l,E=t.default(this._element).closest(ss)[0],T=g.getSelectorFromElement(this._element);if(E){var C=E.nodeName==="UL"||E.nodeName==="OL"?ja:Pa;l=t.default.makeArray(t.default(E).find(C)),l=l[l.length-1]}var N=t.default.Event(es,{relatedTarget:this._element}),M=t.default.Event(as,{relatedTarget:l});if(l&&t.default(l).trigger(N),t.default(this._element).trigger(M),!(M.isDefaultPrevented()||N.isDefaultPrevented())){T&&(i=document.querySelector(T)),this._activate(this._element,E);var R=function(){var z=t.default.Event(ts,{relatedTarget:e._element}),Q=t.default.Event(ns,{relatedTarget:l});t.default(l).trigger(z),t.default(e._element).trigger(Q)};i?this._activate(i,i.parentNode,R):R()}}},u.dispose=function(){t.default.removeData(this._element,_t),this._element=null},u._activate=function(e,i,l){var E=this,T=i&&(i.nodeName==="UL"||i.nodeName==="OL")?t.default(i).find(ja):t.default(i).children(Pa),C=T[0],N=l&&C&&t.default(C).hasClass(Ra),M=function(){return E._transitionComplete(e,C,l)};if(C&&N){var R=g.getTransitionDurationFromElement(C);t.default(C).removeClass(xa).one(g.TRANSITION_END,M).emulateTransitionEnd(R)}else M()},u._transitionComplete=function(e,i,l){if(i){t.default(i).removeClass(qe);var E=t.default(i.parentNode).find(ds)[0];E&&t.default(E).removeClass(qe),i.getAttribute("role")==="tab"&&i.setAttribute("aria-selected",!1)}t.default(e).addClass(qe),e.getAttribute("role")==="tab"&&e.setAttribute("aria-selected",!0),g.reflow(e),e.classList.contains(Ra)&&e.classList.add(xa);var T=e.parentNode;if(T&&T.nodeName==="LI"&&(T=T.parentNode),T&&t.default(T).hasClass(Xr)){var C=t.default(e).closest(rs)[0];if(C){var N=[].slice.call(C.querySelectorAll(os));t.default(N).addClass(qe)}e.setAttribute("aria-expanded",!0)}l&&l()},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this),l=i.data(_t);if(l||(l=new p(this),i.data(_t,l)),typeof e=="string"){if(typeof l[e]>"u")throw new TypeError('No method named "'+e+'"');l[e]()}})},b(p,null,[{key:"VERSION",get:function(){return Fr}}]),p})();t.default(document).on(is,ls,function(p){p.preventDefault(),Qe._jQueryInterface.call(t.default(this),"show")}),t.default.fn[Ye]=Qe._jQueryInterface,t.default.fn[Ye].Constructor=Qe,t.default.fn[Ye].noConflict=function(){return t.default.fn[Ye]=Jr,Qe._jQueryInterface};var Ee="toast",cs="4.6.2",Et="bs.toast",Fe="."+Et,us=t.default.fn[Ee],fs="fade",Ha="hide",ze="show",Va="showing",$a="click.dismiss"+Fe,hs="hide"+Fe,ms="hidden"+Fe,vs="show"+Fe,ps="shown"+Fe,gs='[data-dismiss="toast"]',Ua={animation:!0,autohide:!0,delay:500},_s={animation:"boolean",autohide:"boolean",delay:"number"},yt=(function(){function p(o,e){this._element=o,this._config=this._getConfig(e),this._timeout=null,this._setListeners()}var u=p.prototype;return u.show=function(){var e=this,i=t.default.Event(vs);if(t.default(this._element).trigger(i),!i.isDefaultPrevented()){this._clearTimeout(),this._config.animation&&this._element.classList.add(fs);var l=function(){e._element.classList.remove(Va),e._element.classList.add(ze),t.default(e._element).trigger(ps),e._config.autohide&&(e._timeout=setTimeout(function(){e.hide()},e._config.delay))};if(this._element.classList.remove(Ha),g.reflow(this._element),this._element.classList.add(Va),this._config.animation){var E=g.getTransitionDurationFromElement(this._element);t.default(this._element).one(g.TRANSITION_END,l).emulateTransitionEnd(E)}else l()}},u.hide=function(){if(this._element.classList.contains(ze)){var e=t.default.Event(hs);t.default(this._element).trigger(e),!e.isDefaultPrevented()&&this._close()}},u.dispose=function(){this._clearTimeout(),this._element.classList.contains(ze)&&this._element.classList.remove(ze),t.default(this._element).off($a),t.default.removeData(this._element,Et),this._element=null,this._config=null},u._getConfig=function(e){return e=y({},Ua,t.default(this._element).data(),typeof e=="object"&&e?e:{}),g.typeCheckConfig(Ee,e,this.constructor.DefaultType),e},u._setListeners=function(){var e=this;t.default(this._element).on($a,gs,function(){return e.hide()})},u._close=function(){var e=this,i=function(){e._element.classList.add(Ha),t.default(e._element).trigger(ms)};if(this._element.classList.remove(ze),this._config.animation){var l=g.getTransitionDurationFromElement(this._element);t.default(this._element).one(g.TRANSITION_END,i).emulateTransitionEnd(l)}else i()},u._clearTimeout=function(){clearTimeout(this._timeout),this._timeout=null},p._jQueryInterface=function(e){return this.each(function(){var i=t.default(this),l=i.data(Et),E=typeof e=="object"&&e;if(l||(l=new p(this,E),i.data(Et,l)),typeof e=="string"){if(typeof l[e]>"u")throw new TypeError('No method named "'+e+'"');l[e](this)}})},b(p,null,[{key:"VERSION",get:function(){return cs}},{key:"DefaultType",get:function(){return _s}},{key:"Default",get:function(){return Ua}}]),p})();t.default.fn[Ee]=yt._jQueryInterface,t.default.fn[Ee].Constructor=yt,t.default.fn[Ee].noConflict=function(){return t.default.fn[Ee]=us,yt._jQueryInterface},s.Alert=ue,s.Button=Ie,s.Carousel=he,s.Collapse=xe,s.Dropdown=Z,s.Modal=Ve,s.Popover=vt,s.Scrollspy=Ke,s.Tab=Qe,s.Toast=yt,s.Tooltip=re,s.Util=g,Object.defineProperty(s,"__esModule",{value:!0})}))})(Je,Je.exports)),Je.exports}ml();var St={exports:{}},vl=St.exports,za;function pl(){return za||(za=1,(function(a,n){(function(s,d,h){a.exports=h(),a.exports.default=h()})("slugify",vl,function(){var s=JSON.parse(`{"$":"dollar","%":"percent","&":"and","<":"less",">":"greater","|":"or","¢":"cent","£":"pound","¤":"currency","¥":"yen","©":"(c)","ª":"a","®":"(r)","º":"o","À":"A","Á":"A","Â":"A","Ã":"A","Ä":"A","Å":"A","Æ":"AE","Ç":"C","È":"E","É":"E","Ê":"E","Ë":"E","Ì":"I","Í":"I","Î":"I","Ï":"I","Ð":"D","Ñ":"N","Ò":"O","Ó":"O","Ô":"O","Õ":"O","Ö":"O","Ø":"O","Ù":"U","Ú":"U","Û":"U","Ü":"U","Ý":"Y","Þ":"TH","ß":"ss","à":"a","á":"a","â":"a","ã":"a","ä":"a","å":"a","æ":"ae","ç":"c","è":"e","é":"e","ê":"e","ë":"e","ì":"i","í":"i","î":"i","ï":"i","ð":"d","ñ":"n","ò":"o","ó":"o","ô":"o","õ":"o","ö":"o","ø":"o","ù":"u","ú":"u","û":"u","ü":"u","ý":"y","þ":"th","ÿ":"y","Ā":"A","ā":"a","Ă":"A","ă":"a","Ą":"A","ą":"a","Ć":"C","ć":"c","Č":"C","č":"c","Ď":"D","ď":"d","Đ":"DJ","đ":"dj","Ē":"E","ē":"e","Ė":"E","ė":"e","Ę":"e","ę":"e","Ě":"E","ě":"e","Ğ":"G","ğ":"g","Ģ":"G","ģ":"g","Ĩ":"I","ĩ":"i","Ī":"i","ī":"i","Į":"I","į":"i","İ":"I","ı":"i","Ķ":"k","ķ":"k","Ļ":"L","ļ":"l","Ľ":"L","ľ":"l","Ł":"L","ł":"l","Ń":"N","ń":"n","Ņ":"N","ņ":"n","Ň":"N","ň":"n","Ō":"O","ō":"o","Ő":"O","ő":"o","Œ":"OE","œ":"oe","Ŕ":"R","ŕ":"r","Ř":"R","ř":"r","Ś":"S","ś":"s","Ş":"S","ş":"s","Š":"S","š":"s","Ţ":"T","ţ":"t","Ť":"T","ť":"t","Ũ":"U","ũ":"u","Ū":"u","ū":"u","Ů":"U","ů":"u","Ű":"U","ű":"u","Ų":"U","ų":"u","Ŵ":"W","ŵ":"w","Ŷ":"Y","ŷ":"y","Ÿ":"Y","Ź":"Z","ź":"z","Ż":"Z","ż":"z","Ž":"Z","ž":"z","Ə":"E","ƒ":"f","Ơ":"O","ơ":"o","Ư":"U","ư":"u","ǈ":"LJ","ǉ":"lj","ǋ":"NJ","ǌ":"nj","Ș":"S","ș":"s","Ț":"T","ț":"t","ə":"e","˚":"o","Ά":"A","Έ":"E","Ή":"H","Ί":"I","Ό":"O","Ύ":"Y","Ώ":"W","ΐ":"i","Α":"A","Β":"B","Γ":"G","Δ":"D","Ε":"E","Ζ":"Z","Η":"H","Θ":"8","Ι":"I","Κ":"K","Λ":"L","Μ":"M","Ν":"N","Ξ":"3","Ο":"O","Π":"P","Ρ":"R","Σ":"S","Τ":"T","Υ":"Y","Φ":"F","Χ":"X","Ψ":"PS","Ω":"W","Ϊ":"I","Ϋ":"Y","ά":"a","έ":"e","ή":"h","ί":"i","ΰ":"y","α":"a","β":"b","γ":"g","δ":"d","ε":"e","ζ":"z","η":"h","θ":"8","ι":"i","κ":"k","λ":"l","μ":"m","ν":"n","ξ":"3","ο":"o","π":"p","ρ":"r","ς":"s","σ":"s","τ":"t","υ":"y","φ":"f","χ":"x","ψ":"ps","ω":"w","ϊ":"i","ϋ":"y","ό":"o","ύ":"y","ώ":"w","Ё":"Yo","Ђ":"DJ","Є":"Ye","І":"I","Ї":"Yi","Ј":"J","Љ":"LJ","Њ":"NJ","Ћ":"C","Џ":"DZ","А":"A","Б":"B","В":"V","Г":"G","Д":"D","Е":"E","Ж":"Zh","З":"Z","И":"I","Й":"J","К":"K","Л":"L","М":"M","Н":"N","О":"O","П":"P","Р":"R","С":"S","Т":"T","У":"U","Ф":"F","Х":"H","Ц":"C","Ч":"Ch","Ш":"Sh","Щ":"Sh","Ъ":"U","Ы":"Y","Ь":"","Э":"E","Ю":"Yu","Я":"Ya","а":"a","б":"b","в":"v","г":"g","д":"d","е":"e","ж":"zh","з":"z","и":"i","й":"j","к":"k","л":"l","м":"m","н":"n","о":"o","п":"p","р":"r","с":"s","т":"t","у":"u","ф":"f","х":"h","ц":"c","ч":"ch","ш":"sh","щ":"sh","ъ":"u","ы":"y","ь":"","э":"e","ю":"yu","я":"ya","ё":"yo","ђ":"dj","є":"ye","і":"i","ї":"yi","ј":"j","љ":"lj","њ":"nj","ћ":"c","ѝ":"u","џ":"dz","Ґ":"G","ґ":"g","Ғ":"GH","ғ":"gh","Қ":"KH","қ":"kh","Ң":"NG","ң":"ng","Ү":"UE","ү":"ue","Ұ":"U","ұ":"u","Һ":"H","һ":"h","Ә":"AE","ә":"ae","Ө":"OE","ө":"oe","Ա":"A","Բ":"B","Գ":"G","Դ":"D","Ե":"E","Զ":"Z","Է":"E'","Ը":"Y'","Թ":"T'","Ժ":"JH","Ի":"I","Լ":"L","Խ":"X","Ծ":"C'","Կ":"K","Հ":"H","Ձ":"D'","Ղ":"GH","Ճ":"TW","Մ":"M","Յ":"Y","Ն":"N","Շ":"SH","Չ":"CH","Պ":"P","Ջ":"J","Ռ":"R'","Ս":"S","Վ":"V","Տ":"T","Ր":"R","Ց":"C","Փ":"P'","Ք":"Q'","Օ":"O''","Ֆ":"F","և":"EV","ء":"a","آ":"aa","أ":"a","ؤ":"u","إ":"i","ئ":"e","ا":"a","ب":"b","ة":"h","ت":"t","ث":"th","ج":"j","ح":"h","خ":"kh","د":"d","ذ":"th","ر":"r","ز":"z","س":"s","ش":"sh","ص":"s","ض":"dh","ط":"t","ظ":"z","ع":"a","غ":"gh","ف":"f","ق":"q","ك":"k","ل":"l","م":"m","ن":"n","ه":"h","و":"w","ى":"a","ي":"y","ً":"an","ٌ":"on","ٍ":"en","َ":"a","ُ":"u","ِ":"e","ْ":"","٠":"0","١":"1","٢":"2","٣":"3","٤":"4","٥":"5","٦":"6","٧":"7","٨":"8","٩":"9","پ":"p","چ":"ch","ژ":"zh","ک":"k","گ":"g","ی":"y","۰":"0","۱":"1","۲":"2","۳":"3","۴":"4","۵":"5","۶":"6","۷":"7","۸":"8","۹":"9","฿":"baht","ა":"a","ბ":"b","გ":"g","დ":"d","ე":"e","ვ":"v","ზ":"z","თ":"t","ი":"i","კ":"k","ლ":"l","მ":"m","ნ":"n","ო":"o","პ":"p","ჟ":"zh","რ":"r","ს":"s","ტ":"t","უ":"u","ფ":"f","ქ":"k","ღ":"gh","ყ":"q","შ":"sh","ჩ":"ch","ც":"ts","ძ":"dz","წ":"ts","ჭ":"ch","ხ":"kh","ჯ":"j","ჰ":"h","Ṣ":"S","ṣ":"s","Ẁ":"W","ẁ":"w","Ẃ":"W","ẃ":"w","Ẅ":"W","ẅ":"w","ẞ":"SS","Ạ":"A","ạ":"a","Ả":"A","ả":"a","Ấ":"A","ấ":"a","Ầ":"A","ầ":"a","Ẩ":"A","ẩ":"a","Ẫ":"A","ẫ":"a","Ậ":"A","ậ":"a","Ắ":"A","ắ":"a","Ằ":"A","ằ":"a","Ẳ":"A","ẳ":"a","Ẵ":"A","ẵ":"a","Ặ":"A","ặ":"a","Ẹ":"E","ẹ":"e","Ẻ":"E","ẻ":"e","Ẽ":"E","ẽ":"e","Ế":"E","ế":"e","Ề":"E","ề":"e","Ể":"E","ể":"e","Ễ":"E","ễ":"e","Ệ":"E","ệ":"e","Ỉ":"I","ỉ":"i","Ị":"I","ị":"i","Ọ":"O","ọ":"o","Ỏ":"O","ỏ":"o","Ố":"O","ố":"o","Ồ":"O","ồ":"o","Ổ":"O","ổ":"o","Ỗ":"O","ỗ":"o","Ộ":"O","ộ":"o","Ớ":"O","ớ":"o","Ờ":"O","ờ":"o","Ở":"O","ở":"o","Ỡ":"O","ỡ":"o","Ợ":"O","ợ":"o","Ụ":"U","ụ":"u","Ủ":"U","ủ":"u","Ứ":"U","ứ":"u","Ừ":"U","ừ":"u","Ử":"U","ử":"u","Ữ":"U","ữ":"u","Ự":"U","ự":"u","Ỳ":"Y","ỳ":"y","Ỵ":"Y","ỵ":"y","Ỷ":"Y","ỷ":"y","Ỹ":"Y","ỹ":"y","–":"-","‘":"'","’":"'","“":"\\"","”":"\\"","„":"\\"","†":"+","•":"*","…":"...","₠":"ecu","₢":"cruzeiro","₣":"french franc","₤":"lira","₥":"mill","₦":"naira","₧":"peseta","₨":"rupee","₩":"won","₪":"new shequel","₫":"dong","€":"euro","₭":"kip","₮":"tugrik","₯":"drachma","₰":"penny","₱":"peso","₲":"guarani","₳":"austral","₴":"hryvnia","₵":"cedi","₸":"kazakhstani tenge","₹":"indian rupee","₺":"turkish lira","₽":"russian ruble","₿":"bitcoin","℠":"sm","™":"tm","∂":"d","∆":"delta","∑":"sum","∞":"infinity","♥":"love","元":"yuan","円":"yen","﷼":"rial","ﻵ":"laa","ﻷ":"laa","ﻹ":"lai","ﻻ":"la"}`),d=JSON.parse('{"bg":{"Й":"Y","Ц":"Ts","Щ":"Sht","Ъ":"A","Ь":"Y","й":"y","ц":"ts","щ":"sht","ъ":"a","ь":"y"},"de":{"Ä":"AE","ä":"ae","Ö":"OE","ö":"oe","Ü":"UE","ü":"ue","ß":"ss","%":"prozent","&":"und","|":"oder","∑":"summe","∞":"unendlich","♥":"liebe"},"es":{"%":"por ciento","&":"y","<":"menor que",">":"mayor que","|":"o","¢":"centavos","£":"libras","¤":"moneda","₣":"francos","∑":"suma","∞":"infinito","♥":"amor"},"fr":{"%":"pourcent","&":"et","<":"plus petit",">":"plus grand","|":"ou","¢":"centime","£":"livre","¤":"devise","₣":"franc","∑":"somme","∞":"infini","♥":"amour"},"pt":{"%":"porcento","&":"e","<":"menor",">":"maior","|":"ou","¢":"centavo","∑":"soma","£":"libra","∞":"infinito","♥":"amor"},"uk":{"И":"Y","и":"y","Й":"Y","й":"y","Ц":"Ts","ц":"ts","Х":"Kh","х":"kh","Щ":"Shch","щ":"shch","Г":"H","г":"h"},"vi":{"Đ":"D","đ":"d"},"da":{"Ø":"OE","ø":"oe","Å":"AA","å":"aa","%":"procent","&":"og","|":"eller","$":"dollar","<":"mindre end",">":"større end"},"nb":{"&":"og","Å":"AA","Æ":"AE","Ø":"OE","å":"aa","æ":"ae","ø":"oe"},"it":{"&":"e"},"nl":{"&":"en"},"sv":{"&":"och","Å":"AA","Ä":"AE","Ö":"OE","å":"aa","ä":"ae","ö":"oe"}}');function h(m,t){if(typeof m!="string")throw new Error("slugify: string argument expected");t=typeof t=="string"?{replacement:t}:t||{};var r=d[t.locale]||{},S=t.replacement===void 0?"-":t.replacement,b=t.trim===void 0?!0:t.trim,y=m.normalize().split("").reduce(function(c,v){var f=r[v];return f===void 0&&(f=s[v]),f===void 0&&(f=v),f===S&&(f=" "),c+f.replace(t.remove||/[^\w\s$*_+~.()'"!\-:@]+/g,"")},"");return t.strict&&(y=y.replace(/[^A-Za-z0-9\s]/g,"")),b&&(y=y.trim()),y=y.replace(/\s+/g,S),t.lower&&(y=y.toLowerCase()),y}return h.extend=function(m){Object.assign(s,m)},h})})(St)),St.exports}var gl=pl();const _l=bs(gl),Ja={kpg:{daily:15,weekly:50,alltime:100}};function El(a,n,s,d){d=d||function(A,O,k,w,D){var g=O.split(`
`),I=Math.max(w-3,0),P=Math.min(g.length,w+3),j=D(k),x=g.slice(I,P).map(function($,K){var F=K+I+1;return(F==w?" >> ":"    ")+F+"| "+$}).join(`
`);throw A.path=j,A.message=(j||"ejs")+":"+w+`
`+x+`

`+A.message,A},n=n||function(_){return _==null?"":String(_).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(_){return h[_]||_}var r=1,S=`<table id='leaderboard-table'>
<thead>
<tr class='leaderboard-headers'>
<th class='header-rank' scope="col" data-l10n='stats-rank' data-caps='true'>RANK</th>
<th class='header-player' scope="col" data-l10n='stats-player' data-caps='true'>PLAYER</th>
<!--
<th class='header-active' scope="col" data-l10n='stats-active' data-caps='true'>ACTIVE</th>
-->
<th class='header-stat' scope="col" data-l10n='<%= env.statName %>' data-caps='true'>STAT</th>
<% if (env.type != 'most_kills' && env.type != 'win_streak') { %>
<th class='header-games' scope="col" data-l10n='stats-games' data-caps='true'>GAMES (><%= env.minGames %>)</th>
<% } %>
<th class='header-region' scope="col" data-l10n='stats-region' data-caps='true'>REGION</th>
</tr>
</thead>
<tbody class='leaderboard-values'>
<% for (var i = 0, data = env.data; i < data.length; i++) { %>
<% if (Array.isArray(data[i].slugs)) { %>
<tr class='main multiple-players'>
<td class='data-rank' scope="row">#<%= i + 1 %></td>
<td class='data-player-names'>
<% for (var j = 0; j < data[i].slugs.length; j++) { %>
<span class='player-name'>
<% if (data[i].slugs[j]) { %>
<a href="/stats/?slug=<%= data[i].slugs[j] %>"><%= data[i].usernames[j] %></a>
<% } else { %>
<%= data[i].usernames[j] %>
<% } %>
</span>
<% } %>
</td>
<td><%= data[i].val %></td>
<td><%= data[i].region ? data[i].region.toUpperCase() : '' %></td>
<!--
<td class='<%= data[i].active ? 'active' : 'inactive' %>'></td>
-->
</tr>
<% } else { %>
<tr class='main single-player'>
<td class='data-rank' scope="row">#<%= i + 1 %></td>
<td class='data-player-names'>
<span class='player-name'>
<% if (data[i].slug) { %>
<a href="/stats/?slug=<%= data[i].slug%>"><%= data[i].username %></a>
<% } else { %>
<%= data[i].username %>
<% } %>
</span>
</td>
<!--
<td class='<%= data[i].active ? 'active' : 'inactive' %>'></td>
-->
<td><%= data[i].val %></td>
<% if (env.type != 'most_kills' && env.type != 'win_streak') { %>
<td><%= data[i].games %></td>
<% } %>
<td class='data-region'><%= data[i].region ? data[i].region.toUpperCase() : '' %></td>
</tr>
<% } %>
<% } %>
</tbody>
</table>`,b="../src/stats/js/templates/leaderboard.ejs";try{let _=function(A){A!=null&&(y+=A)};var y="";_(`<table id='leaderboard-table'>
<thead>
<tr class='leaderboard-headers'>
<th class='header-rank' scope="col" data-l10n='stats-rank' data-caps='true'>RANK</th>
<th class='header-player' scope="col" data-l10n='stats-player' data-caps='true'>PLAYER</th>
<!--
<th class='header-active' scope="col" data-l10n='stats-active' data-caps='true'>ACTIVE</th>
-->
<th class='header-stat' scope="col" data-l10n='`),r=9,_(n(a.statName)),_(`' data-caps='true'>STAT</th>
`),r=10,a.type!="most_kills"&&a.type!="win_streak"&&(_(`
<th class='header-games' scope="col" data-l10n='stats-games' data-caps='true'>GAMES (>`),r=11,_(n(a.minGames)),_(`)</th>
`),r=12),_(`
<th class='header-region' scope="col" data-l10n='stats-region' data-caps='true'>REGION</th>
</tr>
</thead>
<tbody class='leaderboard-values'>
`),r=17;for(var c=0,v=a.data;c<v.length;c++){if(_(`
`),r=18,Array.isArray(v[c].slugs)){_(`
<tr class='main multiple-players'>
<td class='data-rank' scope="row">#`),r=20,_(n(c+1)),_(`</td>
<td class='data-player-names'>
`),r=22;for(var f=0;f<v[c].slugs.length;f++)_(`
<span class='player-name'>
`),r=24,v[c].slugs[f]?(_(`
<a href="/stats/?slug=`),r=25,_(n(v[c].slugs[f])),_('">'),_(n(v[c].usernames[f])),_(`</a>
`),r=26):(_(`
`),r=27,_(n(v[c].usernames[f])),_(`
`),r=28),_(`
</span>
`),r=30;_(`
</td>
<td>`),r=32,_(n(v[c].val)),_(`</td>
<td>`),r=33,_(n(v[c].region?v[c].region.toUpperCase():"")),_(`</td>
<!--
<td class='`),r=35,_(n(v[c].active?"active":"inactive")),_(`'></td>
-->
</tr>
`),r=38}else _(`
<tr class='main single-player'>
<td class='data-rank' scope="row">#`),r=40,_(n(c+1)),_(`</td>
<td class='data-player-names'>
<span class='player-name'>
`),r=43,v[c].slug?(_(`
<a href="/stats/?slug=`),r=44,_(n(v[c].slug)),_('">'),_(n(v[c].username)),_(`</a>
`),r=45):(_(`
`),r=46,_(n(v[c].username)),_(`
`),r=47),_(`
</span>
</td>
<!--
<td class='`),r=51,_(n(v[c].active?"active":"inactive")),_(`'></td>
-->
<td>`),r=53,_(n(v[c].val)),_(`</td>
`),r=54,a.type!="most_kills"&&a.type!="win_streak"&&(_(`
<td>`),r=55,_(n(v[c].games)),_(`</td>
`),r=56),_(`
<td class='data-region'>`),r=57,_(n(v[c].region?v[c].region.toUpperCase():"")),_(`</td>
</tr>
`),r=59;_(`
`),r=60}return _(`
</tbody>
</table>`),r=62,y}catch(_){d(_,S,b,r,n)}}function yl(a,n,s,d){d=d||function(v,f,_,A,O){var k=f.split(`
`),w=Math.max(A-3,0),D=Math.min(k.length,A+3),g=O(_),I=k.slice(w,D).map(function(P,j){var x=j+w+1;return(x==A?" >> ":"    ")+x+"| "+P}).join(`
`);throw v.path=g,v.message=(g||"ejs")+":"+A+`
`+I+`

`+v.message,v},n=n||function(c){return c==null?"":String(c).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(c){return h[c]||c}var r=1,S=`<div class="leaderboard-error">
<h2>Unable to load, please try again.</h2>
</div>`,b="../src/stats/js/templates/leaderboardError.ejs";try{let c=function(v){v!=null&&(y+=v)};var y="";return c(`<div class="leaderboard-error">
<h2>Unable to load, please try again.</h2>
</div>`),r=3,y}catch(c){d(c,S,b,r,n)}}function vn(a,n,s,d){d=d||function(v,f,_,A,O){var k=f.split(`
`),w=Math.max(A-3,0),D=Math.min(k.length,A+3),g=O(_),I=k.slice(w,D).map(function(P,j){var x=j+w+1;return(x==A?" >> ":"    ")+x+"| "+P}).join(`
`);throw v.path=g,v.message=(g||"ejs")+":"+A+`
`+I+`

`+v.message,v},n=n||function(c){return c==null?"":String(c).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(c){return h[c]||c}var r=1,S=`<% switch (env.type) {
case 'leaderboard': %>
<div class="col-12 spinner-wrapper-leaderboard">
<div class="spinner"></div>
</div>
<% break; %>
<% case 'player': %>
<div class='container'>
<div class="col-12 spinner-wrapper-player">
<div class="spinner"></div>
</div>
</div>
<% break; %>
<% case 'match_history': %>
<div class="col-12 spinner-wrapper-match-history">
<div class="spinner"></div>
</div>
<% break; %>
<% } %>`,b="../src/stats/js/templates/loading.ejs";try{let c=function(v){v!=null&&(y+=v)};var y="";switch(a.type){case"leaderboard":r=2,c(`
<div class="col-12 spinner-wrapper-leaderboard">
<div class="spinner"></div>
</div>
`),r=6;break;case"player":c(`
<div class='container'>
<div class="col-12 spinner-wrapper-player">
<div class="spinner"></div>
</div>
</div>
`),r=13;break;case"match_history":c(`
<div class="col-12 spinner-wrapper-match-history">
<div class="spinner"></div>
</div>
`),r=18;break}return y}catch(c){d(c,S,b,r,n)}}function Tl(a,n,s,d){d=d||function(f,_,A,O,k){var w=_.split(`
`),D=Math.max(O-3,0),g=Math.min(w.length,O+3),I=k(A),P=w.slice(D,g).map(function(j,x){var $=x+D+1;return($==O?" >> ":"    ")+$+"| "+j}).join(`
`);throw f.path=I,f.message=(I||"ejs")+":"+O+`
`+P+`

`+f.message,f},n=n||function(v){return v==null?"":String(v).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(v){return h[v]||v}var r=1,S=`<!-- Background -->
<div id='leaderboard-bg' class='stats-bg'></div>
<!-- Top ad -->
<% if (!env.phoneDetected) { %>
<div id='ad-block-top' class='container mt-3'>
<div class='ad-block-top-leaderboard'>
<div id='surviv-io_728x90_Leaderboard'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_728x90_Leaderboard'); });
<\/script> -->
</div>
</div>
<div class='ad-block-top-med-rect'>
<div id='surviv-io_300x250_leaderboard'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_leaderboard'); });
<\/script> -->
</div>
</div>
</div>
<% } %>
<!-- Overview Card -->
<div class="container mt-3">
<div class="card card-leaderboard col-lg-8 col-12 p-0">
<div class="card-body">
<div class='row card-row-top'>
<div class='col-12'>
<div class="leaderboard-title ml-sm-3 ml-0 mr-0 mt-3" data-l10n='index-leaderboards' data-caps='true'>LEADERBOARDS</div>
</div>
</div>
</div>
</div>
</div>
<!-- Mode selectors -->
<div class='container mt-3'>
<div class="row">
<div class='col-lg-2 col-3 pr-lg-3 pr-1'>
<select id="leaderboard-team-mode" class="leaderboard-opt custom-select">
<option value="solo" data-l10n='stats-solo'>Solo</option>
<option value="duo" data-l10n='stats-duo'>Duo</option>
<option value="squad" data-l10n='stats-squad'>Squad</option>
</select>
</div>
<div class='col-lg-2 col-3 pl-lg-0 pr-lg-3 pl-0 pr-1'>
<select id="leaderboard-type" class="leaderboard-opt custom-select">
<option value="most_kills" data-l10n='stats-most-kills'>Most kills</option>
<option value="most_damage_dealt" data-l10n='stats-most-damage'>Most damage</option>
<option value="kpg" data-l10n='stats-kpg-full'>Kills per game</option>
<option value="kills" data-l10n='stats-total-kills'>Total kills</option>
<option value="wins" data-l10n='stats-total-wins'>Total wins</option>
</select>
</div>
<div class='col-lg-2 col-3 pl-lg-0 pr-lg-3 pl-0 pr-1'>
<select id="leaderboard-time" class="leaderboard-opt custom-select">
<option value="daily" data-l10n='stats-today'>Today</option>
<option value="weekly" data-l10n='stats-this-week'>This week</option>
<option value="alltime" data-l10n='stats-all-time'>All time</option>
</select>
</div>
<div class='col-lg-2 col-3 pl-0'>
<select id="leaderboard-map-id" class="leaderboard-opt custom-select">
<% for (var i = 0; i < env.gameModes.length; i++) { %>
<option value="<%= env.gameModes[i].mapId %>"><%= env.gameModes[i].desc.name%></option>
<% } %>
</select>
</div>
</div>
</div>
<div class='container mt-2 mb-4 p-sm-3 p-0'>
<div class="row justify-content-center">
<div class="col-md-12">
<div class="content"></div>
</div>
</div>
</div>
<% if (env.phoneDetected) { %>
<div class='col-12'>
<div class='ad-block-bot-med-rect'>
<div id='surviv-io_300x250_leaderboard'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_leaderboard'); });
<\/script> -->
</div>
</div>
</div>
<% } %>`,b="../src/stats/js/templates/main.ejs";try{let v=function(f){f!=null&&(y+=f)};var y="";v(`<!-- Background -->
<div id='leaderboard-bg' class='stats-bg'></div>
<!-- Top ad -->
`),r=4,a.phoneDetected||(v(`
<div id='ad-block-top' class='container mt-3'>
<div class='ad-block-top-leaderboard'>
<div id='surviv-io_728x90_Leaderboard'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_728x90_Leaderboard'); });
<\/script> -->
</div>
</div>
<div class='ad-block-top-med-rect'>
<div id='surviv-io_300x250_leaderboard'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_leaderboard'); });
<\/script> -->
</div>
</div>
</div>
`),r=21),v(`
<!-- Overview Card -->
<div class="container mt-3">
<div class="card card-leaderboard col-lg-8 col-12 p-0">
<div class="card-body">
<div class='row card-row-top'>
<div class='col-12'>
<div class="leaderboard-title ml-sm-3 ml-0 mr-0 mt-3" data-l10n='index-leaderboards' data-caps='true'>LEADERBOARDS</div>
</div>
</div>
</div>
</div>
</div>
<!-- Mode selectors -->
<div class='container mt-3'>
<div class="row">
<div class='col-lg-2 col-3 pr-lg-3 pr-1'>
<select id="leaderboard-team-mode" class="leaderboard-opt custom-select">
<option value="solo" data-l10n='stats-solo'>Solo</option>
<option value="duo" data-l10n='stats-duo'>Duo</option>
<option value="squad" data-l10n='stats-squad'>Squad</option>
</select>
</div>
<div class='col-lg-2 col-3 pl-lg-0 pr-lg-3 pl-0 pr-1'>
<select id="leaderboard-type" class="leaderboard-opt custom-select">
<option value="most_kills" data-l10n='stats-most-kills'>Most kills</option>
<option value="most_damage_dealt" data-l10n='stats-most-damage'>Most damage</option>
<option value="kpg" data-l10n='stats-kpg-full'>Kills per game</option>
<option value="kills" data-l10n='stats-total-kills'>Total kills</option>
<option value="wins" data-l10n='stats-total-wins'>Total wins</option>
</select>
</div>
<div class='col-lg-2 col-3 pl-lg-0 pr-lg-3 pl-0 pr-1'>
<select id="leaderboard-time" class="leaderboard-opt custom-select">
<option value="daily" data-l10n='stats-today'>Today</option>
<option value="weekly" data-l10n='stats-this-week'>This week</option>
<option value="alltime" data-l10n='stats-all-time'>All time</option>
</select>
</div>
<div class='col-lg-2 col-3 pl-0'>
<select id="leaderboard-map-id" class="leaderboard-opt custom-select">
`),r=62;for(var c=0;c<a.gameModes.length;c++)v(`
<option value="`),r=63,v(n(a.gameModes[c].mapId)),v('">'),v(n(a.gameModes[c].desc.name)),v(`</option>
`),r=64;return v(`
</select>
</div>
</div>
</div>
<div class='container mt-2 mb-4 p-sm-3 p-0'>
<div class="row justify-content-center">
<div class="col-md-12">
<div class="content"></div>
</div>
</div>
</div>
`),r=76,a.phoneDetected&&(v(`
<div class='col-12'>
<div class='ad-block-bot-med-rect'>
<div id='surviv-io_300x250_leaderboard'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_leaderboard'); });
<\/script> -->
</div>
</div>
</div>
`),r=86),y}catch(v){d(v,S,b,r,n)}}const bt={loading:vn,main:Tl,leaderboard:El,leaderboardError:yl};class bl{constructor(n){this.app=n,this.app=n,this.el.find(".leaderboard-opt").change(()=>{this.onChangedParams()})}loading=!1;error=!1;data={};el=L(bt.main({phoneDetected:Ct.mobile&&!Ct.tablet,gameModes:W.getGameModes()}));load(){this.loading=!0,this.error=!1;let n=W.getParameterByName("type")||"most_kills";const s=W.getParameterByName("t")||"daily",d=W.getParameterByName("team")||"solo",h=W.getParameterByName("mapId")||"0";n=="most_kills"&&Number(h)==3&&(n="most_damage_dealt");const m={type:n,interval:s,teamMode:d,mapId:h};L.ajax({url:Xa.resolveUrl("/api/leaderboard"),type:"POST",data:JSON.stringify(m),contentType:"application/json; charset=utf-8",success:t=>{this.data={type:n,interval:s,teamMode:d,mapId:h,data:t}},error:()=>{this.error=!0},complete:()=>{this.loading=!1,this.render()}}),this.render()}onChangedParams(){const n=L("#leaderboard-type").val(),s=L("#leaderboard-time").val(),d=L("#leaderboard-team-mode").val(),h=L("#leaderboard-map-id").val();window.history.pushState("","",`?type=${n}&team=${d}&t=${s}&mapId=${h}`),this.load()}render(){const n={most_kills:"stats-most-kills",most_damage_dealt:"stats-most-damage",kills:"stats-total-kills",wins:"stats-total-wins",kpg:"stats-kpg"};let s="";if(this.loading)s=bt.loading({type:"leaderboard"});else if(this.error||!this.data.data)s=bt.leaderboardError({});else{const d=n[this.data.type]||"";let h=Ja[this.data.type]?Ja[this.data.type][this.data.interval]:1;h=h||1,s=bt.leaderboard({...this.data,statName:d,minGames:h}),L("#leaderboard-team-mode").val(this.data.teamMode),L("#leaderboard-map-id").val(this.data.mapId),L("#leaderboard-type").val(this.data.type),L("#leaderboard-time").val(this.data.interval),Number(this.data.mapId)==3?L('#leaderboard-type option[value="most_kills"]').attr("disabled","disabled"):L('#leaderboard-type option[value="most_kills"]').removeAttr("disabled")}this.el.find(".content").html(s),this.app.localization.localizeIndex()}}const pn=7;Nt({slug:Xt(),offset:Ba(),count:Ba(),teamModeFilter:Ss([Tt(Se.Solo),Tt(Se.Duo),Tt(Se.Squad),Tt(pn)])});Nt({gameId:Xt()});const gn="-1",_n=Object.values(Cs).filter(a=>typeof a=="number").map(a=>a.toString());Nt({interval:be(["daily","weekly","alltime"]),slug:Xt().min(1),mapIdFilter:be([gn,..._n])});const Sl={solo:Se.Solo,duo:Se.Duo,squad:Se.Squad};Nt({interval:be(["daily","weekly","alltime"]),mapId:be(_n).transform(a=>Number(a)),type:be(["most_kills","most_damage_dealt","kpg","kills","wins"]),teamMode:be(["solo","duo","squad"]).transform(a=>Sl[a])});function Cl(a,n,s,d){d=d||function(D,g,I,P,j){var x=g.split(`
`),$=Math.max(P-3,0),K=Math.min(x.length,P+3),F=j(I),Oe=x.slice($,K).map(function(de,De){var ce=De+$+1;return(ce==P?" >> ":"    ")+ce+"| "+de}).join(`
`);throw D.path=F,D.message=(F||"ejs")+":"+P+`
`+Oe+`

`+D.message,D},n=n||function(w){return w==null?"":String(w).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(w){return h[w]||w}var r=1,S=`<% if (env.loading) { %>
<!-- Loading game data -->
<div class="col-12 spinner-wrapper-match-data">
<div class="spinner"></div>
</div>
<% } else if (env.error || !env.data || env.data.length == 0) {%>
<!-- Error loading data -->
<div class='col-lg-10'>
<div class='m-3'>Error loading content, please try again.</div>
</div>
<% } else { %>
<div class='match-header-wrapper'>
<table class='match-table'>
<thead>
<tr class='match-headers'>
<th class='match-header-rank' scope="col" data-l10n='stats-rank' data-caps='true'>RANK</th>
<th class='match-header-icon hide-xs' scope="col"></th>
<th class='match-header-player' scope="col" data-l10n='stats-player' data-caps='true'>PLAYER</th>
<th class='match-header-stat' scope="col" data-l10n='stats-kills' data-caps='true'>KILLS</th>
<th class='match-header-stat hide-xs' scope="col" data-l10n='stats-damage' data-caps='true'>DAMAGE</th>
<th class='match-header-stat' scope="col" data-l10n='stats-survived' data-caps='true'>SURVIVED</th>
</tr>
</thead>
</table>
</div>
<div class='match-table-wrapper'>
<table class='match-table'>
<thead>
<tr class='match-headers'>
<th class='match-header-rank'></th>
<th class='match-header-icon hide-xs'></th>
<th class='match-header-player'></th>
<th class='match-header-stat'></th>
<th class='match-header-stat hide-xs'></th>
<th class='match-header-stat'></th>
</tr>
</thead>
<tbody class='match-values'>
<% var team_id = 0;
var teamIdx = 0; %>
<% for (var i = 0; i < env.data.length; i++) { %>
<%
var d = env.data[i];
var showRank = false;
if (team_id != d.team_id) {
team_id = d.team_id;
teamIdx += 1;
showRank = true;
}
%>
<tr class='main single-player <%= teamIdx % 2 == 0 ? 'match-row-dark' : 'match-row-light' %> <%= d.player_id == env.localId ? 'match-row-local' : '' %>'>
<% if (showRank) { %>
<td class='data-rank' scope="row">#<%= d.rank %></td>
<% } else { %>
<td></td>
<% } %>
<td class='data-player-status hide-xs'>
<% if (env.localId != 0 && d.killer_id == env.localId) { %>
<div class='player-icon player-kill'></div>
<% } %>
<% var killed_ids = d.killed_ids || []; %>
<% for (var j = 0; j < killed_ids.length; j++) { %>
<% if (env.localId != 0 && killed_ids[j] == env.localId) { %>
<div class='player-icon player-death'></div>
<% break %>
<% } %>
<% } %>
</td>
<td class='data-player-names'>
<span class='player-name'>
<% if (d.slug) { %>
<a class='player-slug' href="/stats/?slug=<%= d.slug %>"><%= d.username %></a>
<% } else { %>
<%= d.username %>
<% } %>
</span>
</td>
<td><%= d.kills %></td>
<td class='hide-xs'><%= d.damage_dealt %></td>
<td>
<%= env.formatTime(d.time_alive) %>
</td>
</tr>
<% } %>
</tbody>
</table>
</div>
<% } %>`,b="../src/stats/js/templates/matchData.ejs";try{let w=function(D){D!=null&&(y+=D)};var y="";if(a.loading)w(`
<!-- Loading game data -->
<div class="col-12 spinner-wrapper-match-data">
<div class="spinner"></div>
</div>
`),r=6;else if(a.error||!a.data||a.data.length==0)w(`
<!-- Error loading data -->
<div class='col-lg-10'>
<div class='m-3'>Error loading content, please try again.</div>
</div>
`),r=11;else{w(`
<div class='match-header-wrapper'>
<table class='match-table'>
<thead>
<tr class='match-headers'>
<th class='match-header-rank' scope="col" data-l10n='stats-rank' data-caps='true'>RANK</th>
<th class='match-header-icon hide-xs' scope="col"></th>
<th class='match-header-player' scope="col" data-l10n='stats-player' data-caps='true'>PLAYER</th>
<th class='match-header-stat' scope="col" data-l10n='stats-kills' data-caps='true'>KILLS</th>
<th class='match-header-stat hide-xs' scope="col" data-l10n='stats-damage' data-caps='true'>DAMAGE</th>
<th class='match-header-stat' scope="col" data-l10n='stats-survived' data-caps='true'>SURVIVED</th>
</tr>
</thead>
</table>
</div>
<div class='match-table-wrapper'>
<table class='match-table'>
<thead>
<tr class='match-headers'>
<th class='match-header-rank'></th>
<th class='match-header-icon hide-xs'></th>
<th class='match-header-player'></th>
<th class='match-header-stat'></th>
<th class='match-header-stat hide-xs'></th>
<th class='match-header-stat'></th>
</tr>
</thead>
<tbody class='match-values'>
`),r=39;var c=0,v=0;r=40,w(`
`),r=41;for(var f=0;f<a.data.length;f++){w(`
`),r=42;var _=a.data[f],A=!1;c!=_.team_id&&(c=_.team_id,v+=1,A=!0),r=50,w(`
<tr class='main single-player `),r=51,w(n(v%2==0?"match-row-dark":"match-row-light")),w(" "),w(n(_.player_id==a.localId?"match-row-local":"")),w(`'>
`),r=52,A?(w(`
<td class='data-rank' scope="row">#`),r=53,w(n(_.rank)),w(`</td>
`),r=54):(w(`
<td></td>
`),r=56),w(`
<td class='data-player-status hide-xs'>
`),r=58,a.localId!=0&&_.killer_id==a.localId&&(w(`
<div class='player-icon player-kill'></div>
`),r=60),w(`
`),r=61;var O=_.killed_ids||[];w(`
`),r=62;for(var k=0;k<O.length;k++){if(w(`
`),r=63,a.localId!=0&&O[k]==a.localId){w(`
<div class='player-icon player-death'></div>
`),r=65;break}w(`
`),r=67}w(`
</td>
<td class='data-player-names'>
<span class='player-name'>
`),r=71,_.slug?(w(`
<a class='player-slug' href="/stats/?slug=`),r=72,w(n(_.slug)),w('">'),w(n(_.username)),w(`</a>
`),r=73):(w(`
`),r=74,w(n(_.username)),w(`
`),r=75),w(`
</span>
</td>
<td>`),r=78,w(n(_.kills)),w(`</td>
<td class='hide-xs'>`),r=79,w(n(_.damage_dealt)),w(`</td>
<td>
`),r=81,w(n(a.formatTime(_.time_alive))),w(`
</td>
</tr>
`),r=84}w(`
</tbody>
</table>
</div>
`),r=88}return y}catch(w){d(w,S,b,r,n)}}function wl(a,n,s,d){d=d||function(I,P,j,x,$){var K=P.split(`
`),F=Math.max(x-3,0),Oe=Math.min(K.length,x+3),de=$(j),De=K.slice(F,Oe).map(function(ce,Dt){var tt=Dt+F+1;return(tt==x?" >> ":"    ")+tt+"| "+ce}).join(`
`);throw I.path=de,I.message=(de||"ejs")+":"+x+`
`+De+`

`+I.message,I},n=n||function(g){return g==null?"":String(g).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(g){return h[g]||g}var r=1,S=`<div class='header-extra'>MATCH HISTORY</div>
<% if (env.error) { %>
<div class='col-lg-10'>
<div class="m-3">Error loading content, please try again.</div>
</div>
<% } else if (env.games.length == 0) { %>
<div class='col-lg-10'>
<div class="m-3">No recent games played.</div>
</div>
<% } else { %>
<div class='col-lg-12'>
<% for (var i = 0; i < env.games.length; i++) { %>
<div class='row row-match match-link js-match-data <%= env.games[i].expanded ? 'match-link-expanded' : '' %>' data-game-id='<%= env.games[i].summary.guid %>'>
<div class='match-link-mode-color match-link-mode-<%= env.games[i].summary.team_mode %>'></div>
<div class='hide-xs col-2'>
<div class='match-link-player-icons'>
<% for (var j = 0; j < env.games[i].summary.team_count; j++) { %>
<div class='match-link-player-icon'></div>
<% } %>
</div>
<div class='match-link-start-time'>
<%
var timeDiff = '';
var timeStart = new Date(env.games[i].summary.end_time);
var now = Date.now();
var secondsPast = (now - timeStart.getTime()) / 1000;
if (secondsPast < 3600) {
var minutes = Math.round(secondsPast/60);
timeDiff = minutes < 2 ? '1 minute ago' : minutes + ' minutes ago';
} else if (secondsPast <= 86400) {
var hours = Math.round(secondsPast/3600);
timeDiff = hours == 1 ? 'an hour ago' : hours + ' hours ago';
} else if (secondsPast > 86400 && secondsPast < 172800) {
timeDiff = Math.floor(secondsPast/86400) + ' day ago';
} else if (secondsPast > 86400) {
timeDiff = Math.floor(secondsPast/86400) + ' days ago';
}
%>
<%= timeDiff %>
</div>
</div>
<div class='col-3'>
<div class='match-link-stat'>
<%
var modeText = env.games[i].summary.team_mode;
modeText = modeText.charAt(0).toUpperCase() + modeText.slice(1);
%>
<div class='match-link-stat-name match-link-stat-name-lg'><%= modeText %> Rank</div>
<div class='match-link-stat-value match-link-stat-value-lg'>
<span class='match-link-stat-rank match-link-stat-<%= env.games[i].summary.rank %>'>#<%= env.games[i].summary.rank %></span>
/<%= env.games[i].summary.team_total || 80 %>
</div>
</div>
</div>
<div class='col-2 col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Kills</div>
<div class='match-link-stat-value match-link-stat-value-md'><%= env.games[i].summary.kills %></div>
</div>
</div>
<% if (env.games[i].summary.team_mode != 'solo') { %>
<div class='hide-xs col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Team Kills</div>
<div class='match-link-stat-value match-link-stat-value-md'><%= env.games[i].summary.team_kills || 0 %></div>
</div>
</div>
<% } %>
<div class='col-2 col-md-1 <%= env.games[i].summary.team_mode == 'solo' ? 'offset-md-1' : '' %>'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Damage Dealt</div>
<div class='match-link-stat-value match-link-stat-value-md'><%= env.games[i].summary.damage_dealt %></div>
</div>
</div>
<div class='col-2 col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Damage Taken</div>
<div class='match-link-stat-value match-link-stat-value-md'><%= env.games[i].summary.damage_taken %></div>
</div>
</div>
<div class='col-2 col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Survived</div>
<div class='match-link-stat-value match-link-stat-value-md'>
<%= env.formatTime(env.games[i].summary.time_alive) %>
</div>
</div>
</div>
<!-- Game mode icon -->
<div class='hide-xs col-md-1'>
<% if (env.games[i].summary.icon) { %>
<div class='match-link-stat'>
<div class='game-mode-icon' style='background-image: url(/<%= env.games[i].summary.icon %>)'></div>
</div>
<% } %>
</div>
<!-- Expand/Unexpand icon -->
<div class='offset-0 col-1 pl-0 pr-0'>
<div class='match-link-expand <%= env.games[i].expanded ? 'match-link-expand-up' : 'match-link-expand-down' %>'>
</div>
</div>
<% if (env.games[i].expanded) { %>
<div id='match-data' class='col-lg-12'>
<!-- match-data.ejs -->
</div>
<% } %>
</div>
<% } %>
</div>
<% if (env.moreGamesAvailable) { %>
<% if (env.loading) { %>
<!-- Loading more games -->
<div class="col-12 spinner-wrapper-match-data">
<div class="spinner"></div>
</div>
<% } else { %>
<div class='col-12 js-match-load-more btn-darken'>More</div>
<% } %>
<% } %>
<% } %>
</div>`,b="../src/stats/js/templates/matchHistory.ejs";try{let g=function(I){I!=null&&(y+=I)};var y="";if(g(`<div class='header-extra'>MATCH HISTORY</div>
`),r=2,a.error)g(`
<div class='col-lg-10'>
<div class="m-3">Error loading content, please try again.</div>
</div>
`),r=6;else if(a.games.length==0)g(`
<div class='col-lg-10'>
<div class="m-3">No recent games played.</div>
</div>
`),r=10;else{g(`
<div class='col-lg-12'>
`),r=12;for(var c=0;c<a.games.length;c++){g(`
<div class='row row-match match-link js-match-data `),r=13,g(n(a.games[c].expanded?"match-link-expanded":"")),g("' data-game-id='"),g(n(a.games[c].summary.guid)),g(`'>
<div class='match-link-mode-color match-link-mode-`),r=14,g(n(a.games[c].summary.team_mode)),g(`'></div>
<div class='hide-xs col-2'>
<div class='match-link-player-icons'>
`),r=17;for(var v=0;v<a.games[c].summary.team_count;v++)g(`
<div class='match-link-player-icon'></div>
`),r=19;g(`
</div>
<div class='match-link-start-time'>
`),r=22;var f="",_=new Date(a.games[c].summary.end_time),A=Date.now(),O=(A-_.getTime())/1e3;if(O<3600){var k=Math.round(O/60);f=k<2?"1 minute ago":k+" minutes ago"}else if(O<=86400){var w=Math.round(O/3600);f=w==1?"an hour ago":w+" hours ago"}else O>86400&&O<172800?f=Math.floor(O/86400)+" day ago":O>86400&&(f=Math.floor(O/86400)+" days ago");r=38,g(`
`),r=39,g(n(f)),g(`
</div>
</div>
<div class='col-3'>
<div class='match-link-stat'>
`),r=44;var D=a.games[c].summary.team_mode;D=D.charAt(0).toUpperCase()+D.slice(1),r=47,g(`
<div class='match-link-stat-name match-link-stat-name-lg'>`),r=48,g(n(D)),g(` Rank</div>
<div class='match-link-stat-value match-link-stat-value-lg'>
<span class='match-link-stat-rank match-link-stat-`),r=50,g(n(a.games[c].summary.rank)),g("'>#"),g(n(a.games[c].summary.rank)),g(`</span>
/`),r=51,g(n(a.games[c].summary.team_total||80)),g(`
</div>
</div>
</div>
<div class='col-2 col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Kills</div>
<div class='match-link-stat-value match-link-stat-value-md'>`),r=58,g(n(a.games[c].summary.kills)),g(`</div>
</div>
</div>
`),r=61,a.games[c].summary.team_mode!="solo"&&(g(`
<div class='hide-xs col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Team Kills</div>
<div class='match-link-stat-value match-link-stat-value-md'>`),r=65,g(n(a.games[c].summary.team_kills||0)),g(`</div>
</div>
</div>
`),r=68),g(`
<div class='col-2 col-md-1 `),r=69,g(n(a.games[c].summary.team_mode=="solo"?"offset-md-1":"")),g(`'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Damage Dealt</div>
<div class='match-link-stat-value match-link-stat-value-md'>`),r=72,g(n(a.games[c].summary.damage_dealt)),g(`</div>
</div>
</div>
<div class='col-2 col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Damage Taken</div>
<div class='match-link-stat-value match-link-stat-value-md'>`),r=78,g(n(a.games[c].summary.damage_taken)),g(`</div>
</div>
</div>
<div class='col-2 col-md-1'>
<div class='match-link-stat'>
<div class='match-link-stat-name match-link-stat-name-md'>Survived</div>
<div class='match-link-stat-value match-link-stat-value-md'>
`),r=85,g(n(a.formatTime(a.games[c].summary.time_alive))),g(`
</div>
</div>
</div>
<!-- Game mode icon -->
<div class='hide-xs col-md-1'>
`),r=91,a.games[c].summary.icon&&(g(`
<div class='match-link-stat'>
<div class='game-mode-icon' style='background-image: url(/`),r=93,g(n(a.games[c].summary.icon)),g(`)'></div>
</div>
`),r=95),g(`
</div>
<!-- Expand/Unexpand icon -->
<div class='offset-0 col-1 pl-0 pr-0'>
<div class='match-link-expand `),r=99,g(n(a.games[c].expanded?"match-link-expand-up":"match-link-expand-down")),g(`'>
</div>
</div>
`),r=102,a.games[c].expanded&&(g(`
<div id='match-data' class='col-lg-12'>
<!-- match-data.ejs -->
</div>
`),r=106),g(`
</div>
`),r=108}g(`
</div>
`),r=110,a.moreGamesAvailable&&(g(`
`),r=111,a.loading?(g(`
<!-- Loading more games -->
<div class="col-12 spinner-wrapper-match-data">
<div class="spinner"></div>
</div>
`),r=116):(g(`
<div class='col-12 js-match-load-more btn-darken'>More</div>
`),r=118),g(`
`),r=119),g(`
`),r=120}return g(`
</div>`),r=121,y}catch(g){d(g,S,b,r,n)}}function Al(a,n,s,d){d=d||function(v,f,_,A,O){var k=f.split(`
`),w=Math.max(A-3,0),D=Math.min(k.length,A+3),g=O(_),I=k.slice(w,D).map(function(P,j){var x=j+w+1;return(x==A?" >> ":"    ")+x+"| "+P}).join(`
`);throw v.path=g,v.message=(g||"ejs")+":"+A+`
`+I+`

`+v.message,v},n=n||function(c){return c==null?"":String(c).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(c){return h[c]||c}var r=1,S=`<!-- Background -->
<div id='leaderboard-bg' class='stats-bg'></div>
<!-- Top ad -->
<% if (!env.phoneDetected) { %>
<div id='ad-block-top' class='container mt-3'>
<div class='ad-block-top-leaderboard'>
<div id='surviv-io_728x90_playerprofile'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_728x90_playerprofile'); });
<\/script> -->
</div>
</div>
<div class='ad-block-top-med-rect'>
<div id='surviv-io_300x250_playerprofile'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_playerprofile'); });
<\/script> -->
</div>
</div>
</div>
<% } %>
<div class="col-12 p-lg-3 p-0">
<div class="content"></div>
</div>
<% if (env.phoneDetected) { %>
<div class='col-12'>
<div class='ad-block-bot-med-rect'>
<div id='surviv-io_300x250_playerprofile'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_playerprofile'); });
<\/script> -->
</div>
</div>
</div>
<% } %>`,b="../src/stats/js/templates/player.ejs";try{let c=function(v){v!=null&&(y+=v)};var y="";return c(`<!-- Background -->
<div id='leaderboard-bg' class='stats-bg'></div>
<!-- Top ad -->
`),r=4,a.phoneDetected||(c(`
<div id='ad-block-top' class='container mt-3'>
<div class='ad-block-top-leaderboard'>
<div id='surviv-io_728x90_playerprofile'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_728x90_playerprofile'); });
<\/script> -->
</div>
</div>
<div class='ad-block-top-med-rect'>
<div id='surviv-io_300x250_playerprofile'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_playerprofile'); });
<\/script> -->
</div>
</div>
</div>
`),r=21),c(`
<div class="col-12 p-lg-3 p-0">
<div class="content"></div>
</div>
`),r=25,a.phoneDetected&&(c(`
<div class='col-12'>
<div class='ad-block-bot-med-rect'>
<div id='surviv-io_300x250_playerprofile'>
<!-- <script type='text/javascript'>
aiptag.cmd.display.push(function() { aipDisplayTag.display('surviv-io_300x250_playerprofile'); });
<\/script> -->
</div>
</div>
</div>
`),r=35),y}catch(c){d(c,S,b,r,n)}}function Nl(a,n,s,d){d=d||function(_,A,O,k,w){var D=A.split(`
`),g=Math.max(k-3,0),I=Math.min(D.length,k+3),P=w(O),j=D.slice(g,I).map(function(x,$){var K=$+g+1;return(K==k?" >> ":"    ")+K+"| "+x}).join(`
`);throw _.path=P,_.message=(P||"ejs")+":"+k+`
`+j+`

`+_.message,_},n=n||function(f){return f==null?"":String(f).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(f){return h[f]||f}var r=1,S=`<!-- Overview Card -->
<div class="container mt-3">
<div class="card card-player col-lg-8 col-12 p-0">
<div class="card-body">
<div class='row card-row-top'>
<% if (env.error) { %>
<div class='col-lg-10'>
<div class="card-player-name mt-3 ml-3">Error loading content, please try again.</div>
</div>
<% } else if (!env.profile.username) { %>
<div class='col-lg-10'>
<div class="card-player-name mt-3 ml-3">That player doesn't exist.</div>
</div>
<% } else { %>
<div class='col-md-1 col-sm-2 col-3'>
<div class='player-image' style='background-image: url("<%= env.profile.avatarTexture %>")'></div>
</div>
<div class='col-md-5 col-sm-10 col-9'>
<div class='card-player-name ml-md-5 ml-sm-1 ml-xs-1 <%= env.profile.banned ? "" : "mt-3"%>'><%= env.profile.username %></div>
<% if (env.profile.banned) { %>
<div class='card-player-banned ml-md-5' data-l10n='stats-banned'>(Account banned)</div>
<% } %>
</div>
<div class='col-md-6 col-12'>
<table class='player-stats-overview'>
<thead>
<tr>
<th scope="col" data-l10n='stats-wins'>Wins</th>
<th scope="col" data-l10n='stats-kills'>Kills</th>
<th scope="col" data-l10n='stats-games'>Games</th>
<th scope="col" data-l10n='stats-kg'>K/G</th>
</tr>
</thead>
<tbody>
<tr>
<td><%= env.profile.wins %></td>
<td><%= env.profile.kills %></td>
<td><%= env.profile.games %></td>
<td><%= env.profile.kpg %></td>
</tr>
</tbody>
</table>
</div>
<% } %>
</div>
</div>
</div>
</div>
<!-- Season/Region selectors -->
<% if (env.teamModes.length > 0) { %>
<div class='container mt-3'>
<div class='row'>
<div class='col-lg-2 col-6'>
<select value='alltime' id='player-time' class="player-opt custom-select">
<option value="daily" data-l10n='stats-today'>Today</option>
<option value="weekly" data-l10n='stats-this-week'>This week</option>
<option value="alltime" data-l10n='stats-all-time'>All time</option>
</select>
</div>
<div class='col-lg-2 col-6 pl-0'>
<select id="player-map-id" class="player-opt custom-select">
<option value="-1" data-l10n='all'>All modes</option>
<% for (var i = 0; i < env.gameModes.length; i++) { %>
<option value="<%= env.gameModes[i].mapId %>"><%= env.gameModes[i].desc.name%></option>
<% } %>
</select>
</div>
<div class='offset-6 col-2 col-rating-help'>
<div class='rating-help'>What is Rating?<div class='rating-help-desc'><span class='highlight'>This feature coming soon!</span></br>Rating will be based on placement and kills within an individual game mode.</div></div>
</div>
</div>
</div>
<% } %>
<!-- Mode Cards -->
<div class="container mt-3">
<div class='row'>
<% for (var i = 0; i < env.teamModes.length; i++) { %>
<!-- Mode Card -->
<!-- pad the last card -->
<% if (i == env.teamModes.length - 1) { %>
<div class='col-lg-4 col-12'>
<% } else { %>
<div class='col-lg-4 col-12 pr-lg-0'>
<% } %>
<div class="card card-mode card-mode-bg-<%= i %>">
<div class="card-body p-1">
<div class='row card-mode-row-top'>
<div class='col-2 p-0'>
<div class='mode-image mode-image-<%= env.teamModes[i].name %>'></div>
</div>
<div class='col-5 p-0'>
<div class="mode-name mode-name-<%= env.teamModes[i].name %>" data-l10n='stats-<%= env.teamModes[i].name %>' data-caps='true'><%= env.teamModes[i].name.toUpperCase() %></div>
</div>
<div class='col-5 mt-2'>
<% if (env.teamModes[i].games > 0) { %>
<div class="mode-games"><span><%= env.teamModes[i].games %></span> <span data-l10n='stats-games' data-caps='true''>Games</span></div>
<% } %>
</div>
</div>
</div>
</div>
<!-- Show "no games played" if no games played -->
<% if (env.teamModes[i].games == 0) { %>
<div class="card card-mode card-mode-no-games">
<div class='col-12'>No games played.</div>
</div>
<% } else { %>
<div class="card card-mode card-mode-bg-mid">
<div class="card-body p-1">
<div class='row m-1'>
<% for (var j = 0; j < env.teamModes[i].midStats.length; j++) { %>
<div class='col-6 mt-1 mb-1'>
<div class='card-mode-stat-mid'>
<div class='card-mode-stat-name' data-l10n='stats-<%= env.teamModes[i].midStats[j].name %>' data-caps='true'><%= env.teamModes[i].midStats[j].name.toUpperCase() %></div>
<div class='card-mode-stat-value' data-l10n='stats-<%= env.teamModes[i].midStats[j].val %>' data-caps='true'><%= env.teamModes[i].midStats[j].val %></div>
</div>
</div>
<% } %>
</div>
</div>
</div>
<div class="card card-mode card-mode-bg-bot">
<div class="card-body p-1">
<div class='row m-1'>
<% for (var j = 0; j < env.teamModes[i].botStats.length; j++) { %>
<div class='col-6 mt-1 mb-1'>
<div class='card-mode-stat-bot'>
<div class='card-mode-stat-name' data-l10n='stats-<%= env.teamModes[i].botStats[j].name %>' data-caps='true'><%= env.teamModes[i].botStats[j].name.toUpperCase() %></div>
<div class='card-mode-stat-value'><%= env.teamModes[i].botStats[j].val %></div>
</div>
</div>
<% } %>
</div>
</div>
</div>
<% } %>
</div>
<% } %>
</div>
</div>
<!-- Close Mode Cards -->
<!-- Extra Stats -->
<% if (env.profile.username) { %>
<div class="container mt-3">
<div class='row m-0'>
<div class='offset-0 offset-md-8 col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter <%= env.teamModeFilter == 7 ? 'extra-team-mode-filter-selected' : '' %> btn-darken' data-filter='7'>All</div>
</div>
<div class='col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter <%= env.teamModeFilter == 1 ? 'extra-team-mode-filter-selected' : '' %> btn-darken' data-filter='1'>Solo</div>
</div>
<div class='col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter <%= env.teamModeFilter == 2 ? 'extra-team-mode-filter-selected' : '' %>  btn-darken' data-filter='2'>Duo</div>
</div>
<div class='col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter <%= env.teamModeFilter == 4 ? 'extra-team-mode-filter-selected' : '' %> btn-darken' data-filter='4'>Squad</div>
</div>
</div>
</div>
<div class="container mt-3">
<!-- Extra Stats Sort Options -->
<div class='row'>
<div class='offset-8 col-4'>
</div>
</div>
<div class='row'>
<!-- Extra Stats Selectors -->
<div class='col-12 col-md-2'>
<div id='selector-extra-matches' class='extra-matches selector-extra col-2 col-md-12 p-0'>Matches<span class='selected-extra'></span></div>
<!-- <div id='selector-extra-weapons' class='extra-weapons selector-extra'>Weapons</div> -->
<!-- <div id='selector-extra-misc' class='extra-misc selector-extra'>Misc</div> -->
</div>
<!-- Extra Stats Main -->
<div id='match-history' class='col-12 col-md-10'>
<div class='header-extra'>MATCH HISTORY</div>
<div class='row-extra-match'>
</div>
</div>
</div>
</div>
<% } %>
<!-- Close Extra Stats -->`,b="../src/stats/js/templates/playerCards.ejs";try{let f=function(_){_!=null&&(y+=_)};var y="";if(f(`<!-- Overview Card -->
<div class="container mt-3">
<div class="card card-player col-lg-8 col-12 p-0">
<div class="card-body">
<div class='row card-row-top'>
`),r=6,a.error?(f(`
<div class='col-lg-10'>
<div class="card-player-name mt-3 ml-3">Error loading content, please try again.</div>
</div>
`),r=10):a.profile.username?(f(`
<div class='col-md-1 col-sm-2 col-3'>
<div class='player-image' style='background-image: url("`),r=16,f(n(a.profile.avatarTexture)),f(`")'></div>
</div>
<div class='col-md-5 col-sm-10 col-9'>
<div class='card-player-name ml-md-5 ml-sm-1 ml-xs-1 `),r=19,f(n(a.profile.banned?"":"mt-3")),f("'>"),f(n(a.profile.username)),f(`</div>
`),r=20,a.profile.banned&&(f(`
<div class='card-player-banned ml-md-5' data-l10n='stats-banned'>(Account banned)</div>
`),r=22),f(`
</div>
<div class='col-md-6 col-12'>
<table class='player-stats-overview'>
<thead>
<tr>
<th scope="col" data-l10n='stats-wins'>Wins</th>
<th scope="col" data-l10n='stats-kills'>Kills</th>
<th scope="col" data-l10n='stats-games'>Games</th>
<th scope="col" data-l10n='stats-kg'>K/G</th>
</tr>
</thead>
<tbody>
<tr>
<td>`),r=36,f(n(a.profile.wins)),f(`</td>
<td>`),r=37,f(n(a.profile.kills)),f(`</td>
<td>`),r=38,f(n(a.profile.games)),f(`</td>
<td>`),r=39,f(n(a.profile.kpg)),f(`</td>
</tr>
</tbody>
</table>
</div>
`),r=44):(f(`
<div class='col-lg-10'>
<div class="card-player-name mt-3 ml-3">That player doesn't exist.</div>
</div>
`),r=14),f(`
</div>
</div>
</div>
</div>
<!-- Season/Region selectors -->
`),r=50,a.teamModes.length>0){f(`
<div class='container mt-3'>
<div class='row'>
<div class='col-lg-2 col-6'>
<select value='alltime' id='player-time' class="player-opt custom-select">
<option value="daily" data-l10n='stats-today'>Today</option>
<option value="weekly" data-l10n='stats-this-week'>This week</option>
<option value="alltime" data-l10n='stats-all-time'>All time</option>
</select>
</div>
<div class='col-lg-2 col-6 pl-0'>
<select id="player-map-id" class="player-opt custom-select">
<option value="-1" data-l10n='all'>All modes</option>
`),r=63;for(var c=0;c<a.gameModes.length;c++)f(`
<option value="`),r=64,f(n(a.gameModes[c].mapId)),f('">'),f(n(a.gameModes[c].desc.name)),f(`</option>
`),r=65;f(`
</select>
</div>
<div class='offset-6 col-2 col-rating-help'>
<div class='rating-help'>What is Rating?<div class='rating-help-desc'><span class='highlight'>This feature coming soon!</span></br>Rating will be based on placement and kills within an individual game mode.</div></div>
</div>
</div>
</div>
`),r=73}f(`
<!-- Mode Cards -->
<div class="container mt-3">
<div class='row'>
`),r=77;for(var c=0;c<a.teamModes.length;c++){if(f(`
<!-- Mode Card -->
<!-- pad the last card -->
`),r=80,c==a.teamModes.length-1?(f(`
<div class='col-lg-4 col-12'>
`),r=82):(f(`
<div class='col-lg-4 col-12 pr-lg-0'>
`),r=84),f(`
<div class="card card-mode card-mode-bg-`),r=85,f(n(c)),f(`">
<div class="card-body p-1">
<div class='row card-mode-row-top'>
<div class='col-2 p-0'>
<div class='mode-image mode-image-`),r=89,f(n(a.teamModes[c].name)),f(`'></div>
</div>
<div class='col-5 p-0'>
<div class="mode-name mode-name-`),r=92,f(n(a.teamModes[c].name)),f(`" data-l10n='stats-`),f(n(a.teamModes[c].name)),f("' data-caps='true'>"),f(n(a.teamModes[c].name.toUpperCase())),f(`</div>
</div>
<div class='col-5 mt-2'>
`),r=95,a.teamModes[c].games>0&&(f(`
<div class="mode-games"><span>`),r=96,f(n(a.teamModes[c].games)),f(`</span> <span data-l10n='stats-games' data-caps='true''>Games</span></div>
`),r=97),f(`
</div>
</div>
</div>
</div>
<!-- Show "no games played" if no games played -->
`),r=103,a.teamModes[c].games==0)f(`
<div class="card card-mode card-mode-no-games">
<div class='col-12'>No games played.</div>
</div>
`),r=107;else{f(`
<div class="card card-mode card-mode-bg-mid">
<div class="card-body p-1">
<div class='row m-1'>
`),r=111;for(var v=0;v<a.teamModes[c].midStats.length;v++)f(`
<div class='col-6 mt-1 mb-1'>
<div class='card-mode-stat-mid'>
<div class='card-mode-stat-name' data-l10n='stats-`),r=114,f(n(a.teamModes[c].midStats[v].name)),f("' data-caps='true'>"),f(n(a.teamModes[c].midStats[v].name.toUpperCase())),f(`</div>
<div class='card-mode-stat-value' data-l10n='stats-`),r=115,f(n(a.teamModes[c].midStats[v].val)),f("' data-caps='true'>"),f(n(a.teamModes[c].midStats[v].val)),f(`</div>
</div>
</div>
`),r=118;f(`
</div>
</div>
</div>
<div class="card card-mode card-mode-bg-bot">
<div class="card-body p-1">
<div class='row m-1'>
`),r=125;for(var v=0;v<a.teamModes[c].botStats.length;v++)f(`
<div class='col-6 mt-1 mb-1'>
<div class='card-mode-stat-bot'>
<div class='card-mode-stat-name' data-l10n='stats-`),r=128,f(n(a.teamModes[c].botStats[v].name)),f("' data-caps='true'>"),f(n(a.teamModes[c].botStats[v].name.toUpperCase())),f(`</div>
<div class='card-mode-stat-value'>`),r=129,f(n(a.teamModes[c].botStats[v].val)),f(`</div>
</div>
</div>
`),r=132;f(`
</div>
</div>
</div>
`),r=136}f(`
</div>
`),r=138}return f(`
</div>
</div>
<!-- Close Mode Cards -->
<!-- Extra Stats -->
`),r=143,a.profile.username&&(f(`
<div class="container mt-3">
<div class='row m-0'>
<div class='offset-0 offset-md-8 col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter `),r=147,f(n(a.teamModeFilter==7?"extra-team-mode-filter-selected":"")),f(` btn-darken' data-filter='7'>All</div>
</div>
<div class='col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter `),r=150,f(n(a.teamModeFilter==1?"extra-team-mode-filter-selected":"")),f(` btn-darken' data-filter='1'>Solo</div>
</div>
<div class='col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter `),r=153,f(n(a.teamModeFilter==2?"extra-team-mode-filter-selected":"")),f(`  btn-darken' data-filter='2'>Duo</div>
</div>
<div class='col-3 col-md-1 p-0'>
<div class='extra-team-mode-filter `),r=156,f(n(a.teamModeFilter==4?"extra-team-mode-filter-selected":"")),f(` btn-darken' data-filter='4'>Squad</div>
</div>
</div>
</div>
<div class="container mt-3">
<!-- Extra Stats Sort Options -->
<div class='row'>
<div class='offset-8 col-4'>
</div>
</div>
<div class='row'>
<!-- Extra Stats Selectors -->
<div class='col-12 col-md-2'>
<div id='selector-extra-matches' class='extra-matches selector-extra col-2 col-md-12 p-0'>Matches<span class='selected-extra'></span></div>
<!-- <div id='selector-extra-weapons' class='extra-weapons selector-extra'>Weapons</div> -->
<!-- <div id='selector-extra-misc' class='extra-misc selector-extra'>Misc</div> -->
</div>
<!-- Extra Stats Main -->
<div id='match-history' class='col-12 col-md-10'>
<div class='header-extra'>MATCH HISTORY</div>
<div class='row-extra-match'>
</div>
</div>
</div>
</div>
`),r=181),f(`
<!-- Close Extra Stats -->`),r=182,y}catch(f){d(f,S,b,r,n)}}const Te={loading:vn,matchData:Cl,matchHistory:wl,player:Al,playerCards:Nl};function Ol(a,n,s){if(n||!a)return{profile:{},teamModes:[],error:n};const d=ws[a.player_icon],h=d?W.emoteImgToSvg(d.texture):"/img/gui/player-gui.svg";let m=a.slug.toLowerCase();m=m.replace(a.username.toLowerCase(),"");const t=m!=""?`${a.username}#${m}`:a.username,r={username:a.username,slugToShow:t,banned:a.banned,avatarTexture:h,wins:a.wins,kills:a.kills,games:a.games,kpg:a.kpg},S=function(f,_,A){f.push({name:_,val:A})},b=[];for(let v=0;v<a.modes.length;v++){const f=a.modes[v],_=[];S(_,"Rating","-"),S(_,"Rank","-");const A=[];S(A,"Wins",f.wins),S(A,"Win %",f.winPct),S(A,"Kills",f.kills),S(A,"Avg Survived",W.formatTime(f.avgTimeAlive)),S(A,"Most kills",f.mostKills),S(A,"K/G",f.kpg),S(A,"Most damage",f.mostDamage),S(A,"Avg Damage",f.avgDamage),b.push({teamMode:f.teamMode,games:f.games,midStats:_,botStats:A})}const y=Object.keys(Qt);for(let v=0;v<y.length;v++){const f=y[v];b.find(_=>_.teamMode==f)||b.push({teamMode:f,games:0})}b.sort((v,f)=>v.teamMode-f.teamMode);for(let v=0;v<b.length;v++){const f=b[v].teamMode;b[v].name=Qt[f]}const c=W.getGameModes();return{profile:r,error:n,teamModes:b,teamModeFilter:s,gameModes:c}}class qt{inProgress=!1;dataValid=!1;error=!1;args={};data=null;query(n,s,d,h){this.inProgress||(this.inProgress=!0,this.error=!1,L.ajax({url:Xa.resolveUrl(n),type:"POST",data:JSON.stringify(s),contentType:"application/json; charset=utf-8",timeout:10*1e3,success:(m,t,r)=>{this.data=m,this.dataValid=!!m},error:()=>{this.error=!0,this.dataValid=!1},complete:()=>{setTimeout(()=>{this.inProgress=!1,h(this.error,this.data)},d)}}))}}class Dl{constructor(n){this.app=n}games=[];moreGamesAvailable=!0;teamModeFilter=pn;userStats=new qt;userStatsCache={};matchHistory=new qt;matchHistoryCache={};matchData=new qt;el=L(Te.player({phoneDetected:Ct.mobile&&!Ct.tablet}));getUrlParams(){const n=new URLSearchParams(window.location.search),s=n.get("slug")||"",d=n.get("time")||"alltime",h=n.get("mapId")||gn,m=n.get("gameId")||"";return{slug:s,interval:d,mapId:h,gameId:m}}getGameByGameId(n){return this.games.find(s=>s.summary.guid==n)}load(){const n=this.getUrlParams(),s=n.slug,d=n.interval,h=n.mapId;this.loadUserStats(s,d,h),this.loadMatchHistory(s,0,7),this.render()}loadUserStats(n,s,d){const h={slug:n,interval:s,mapIdFilter:d},m=`${s}${d}`;if(this.userStatsCache[m]){const{error:t,data:r}=this.userStatsCache[m];this.userStats.data=r,this.userStats.error=t,this.render();return}this.userStats.query("/api/user_stats",h,0,(t,r)=>{this.userStatsCache[m]={error:t,data:r},this.render()})}loadMatchHistory(n,s,d){const m={slug:n,offset:s,count:10,teamModeFilter:d};if(s===0&&this.matchHistoryCache[d]){this.games=this.matchHistoryCache[d],this.moreGamesAvailable=this.games.length>=10,this.render();return}this.matchHistory.query("/api/match_history",m,0,(t,r)=>{const S=W.getGameModes(),b=r||[];for(let c=0;c<b.length;c++){b[c].team_mode=Qt[b[c].team_mode];const v=S.find(f=>f.mapId==b[c].map_id);b[c].icon=v?v.desc.icon:"",this.games.push({expanded:!1,summary:b[c],data:null,dataError:!1})}s===0&&!this.matchHistoryCache[d]&&(this.matchHistoryCache[d]=this.games),this.moreGamesAvailable=b.length>=10;const y=this.getUrlParams().gameId;if(y){for(const c of this.games)if(!c.expanded&&c.summary.guid===y){c.expanded=!0,this.loadMatchData(y);break}}this.render()})}loadMatchData(n){const s={gameId:n};this.matchData.query("/api/match_data",s,0,(d,h)=>{const m=this.getGameByGameId(n);m&&(m.data=h,m.dataError=d||!h),this.render()})}toggleMatchData(n){const s=this.getGameByGameId(n);if(!s)return;const d=s.expanded;for(let h=0;h<this.games.length;h++)this.games[h].expanded=!1;s.expanded=!d,!s.data&&!s.dataError&&this.loadMatchData(n),this.render(),this.updateSearchParams()}updateSearchParams(){const n=this.getUrlParams().slug,s=L("#player-time").val(),d=L("#player-map-id").val();let h=new URLSearchParams;h.set("slug",n),h.set("time",s),h.set("mapId",d);const m=this.games.find(t=>t.expanded);m&&h.set("gameId",m.summary.guid),window.history.pushState("","",`?${h.toString()}`)}onChangedParams(){this.updateSearchParams();const n=this.getUrlParams();this.loadUserStats(n.slug,n.interval,n.mapId)}render(){const n=this.getUrlParams();let s="";if(this.userStats.inProgress)s=Te.loading({type:"player"});else{const r=Ol(this.userStats.data,this.userStats.error,this.teamModeFilter);s=Te.playerCards(r)}this.el.find(".content").html(s);const d=this.el.find("#player-time");d&&(d.val(n.interval),d.on("change",()=>{this.onChangedParams()}));const h=this.el.find("#player-map-id");h&&(h.val(n.mapId),h.on("change",()=>{this.onChangedParams()}));let m="";this.games.length==0&&this.matchHistory.inProgress?m=Te.loading({type:"match_history"}):m=Te.matchHistory({games:this.games,moreGamesAvailable:this.moreGamesAvailable,loading:this.matchHistory.inProgress,error:this.matchHistory.error,formatTime:W.formatTime});const t=this.el.find("#match-history");if(t){t.html(m),L(".js-match-data").on("click",y=>{L(y.target).is("a")||this.toggleMatchData(L(y.currentTarget).data("game-id"))}),L(".js-match-load-more").on("click",y=>{const c=this.getUrlParams();this.loadMatchHistory(c.slug,this.games.length,this.teamModeFilter),this.render()}),L(".extra-team-mode-filter").on("click",y=>{if(!this.matchHistory.inProgress){const c=this.getUrlParams();this.games=[],this.teamModeFilter=L(y.currentTarget).data("filter"),this.loadMatchHistory(c.slug,0,this.teamModeFilter),this.render()}});const r=this.getUrlParams();let S="";const b=this.games.find(y=>y.expanded);if(b){let y=0;if(b.data)for(let c=0;c<b.data.length;c++){const v=b.data[c];if(r.slug==v.slug){y=v.player_id||0;break}}S=Te.matchData({data:b.data,error:b.dataError,loading:this.matchData.inProgress,localId:y,formatTime:W.formatTime})}if(L("#match-data").html(S),b&&b.summary.guid===r.gameId){const y=document.querySelector(`div[data-game-id="${r.gameId}"]`);y&&y.scrollIntoView()}}this.app.localization.localizeIndex()}}function kl(a,n,s,d){d=d||function(v,f,_,A,O){var k=f.split(`
`),w=Math.max(A-3,0),D=Math.min(k.length,A+3),g=O(_),I=k.slice(w,D).map(function(P,j){var x=j+w+1;return(x==A?" >> ":"    ")+x+"| "+P}).join(`
`);throw v.path=g,v.message=(g||"ejs")+":"+A+`
`+I+`

`+v.message,v},n=n||function(c){return c==null?"":String(c).replace(m,t)};var h={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&#34;","'":"&#39;"},m=/[&<>'"]/g;function t(c){return h[c]||c}var r=1,S=`<a class="nav-link dropdown-toggle" href="#" id="selected-language" role="button" data-toggle="dropdown" aria-haspopup="true" aria-expanded="false"><%= env.code.toUpperCase() %></a>
<div class="dropdown-menu" aria-labelledby="navbarDropdown">
<a class="dropdown-item dropdown-language" href="#" value='en'>English</a>
<a class="dropdown-item dropdown-language" href="#" value='es'>Español</a>
</div>`,b="../src/stats/js/templates/langauge.ejs";try{let c=function(v){v!=null&&(y+=v)};var y="";return c('<a class="nav-link dropdown-toggle" href="#" id="selected-language" role="button" data-toggle="dropdown" aria-haspopup="true" aria-expanded="false">'),c(n(a.code.toUpperCase())),c(`</a>
<div class="dropdown-menu" aria-labelledby="navbarDropdown">
<a class="dropdown-item dropdown-language" href="#" value='en'>English</a>
<a class="dropdown-item dropdown-language" href="#" value='es'>Español</a>
</div>`),r=5,y}catch(c){d(c,S,b,r,n)}}const Il={"@metadata":{"last-updated":"2018-05-26",locale:"en"},"word-order":"svo","index-privacy":"privacy","index-go":"Go","index-leaderboards":"Leaderboards","index-my-stats":"My Stats","index-search-players":"Search Players","index-play-survevio":"Play survev.io!","stats-rank":"Rank","stats-most-kills":"Most kills","stats-total-kills":"Total kills","stats-wins":"Wins","stats-total-wins":"Total wins","stats-top-5-percent":"Top 5 percent","stats-kill-death-ratio":"K/D","stats-today":"Today","stats-this-week":"This week","stats-all-time":"All time","stats-preseason":"Preseason","stats-top-100":"TOP 100","stats-player":"Player","stats-games":"Games","stats-rating":"Rating","stats-win-pct":"Win %","stats-top-5":"Top 5 %","stats-win-streak":"Win streak","stats-kdr":"K/D","stats-kpg":"K/G","stats-kpg-full":"Kills per game","stats-most-damage":"Most damage","stats-avg-damage":"Avg damage","stats-avg-kills":"Avg kills","stats-avg-survived":"Avg survived time","stats-region":"Region","stats-north-america":"North America","stats-europe":"Europe","stats-asia":"Asia","stats-players":"players","stats-solo":"Solo","stats-duo":"Duo","stats-squad":"Squad","stats-solo-rank":"Solo Rank","stats-duo-rank":"Duo Rank","stats-squad-rank":"Squad Rank","stats-team-kills":"Team Kills","stats-kill":"Kill","stats-kills":"Kills","stats-damage-dealt":"Damage Dealt","stats-damage-taken":"Damage Taken","stats-survived":"survived","stats-banned":"(Account banned)","game-backpack00":"Pouch","game-backpack01":"Small Pack","game-backpack02":"Regular Pack","game-backpack03":"Military Pack","game-bandage":"Bandage","game-healthkit":"Med Kit","game-soda":"Soda","game-painkiller":"Pills","game-9mm":"9mm","game-12gauge":"12 gauge","game-762mm":"7.62mm","game-556mm":"5.56mm","game-50AE":".50 AE","game-chest01":"Level 1 Vest","game-chest02":"Level 2 Vest","game-chest03":"Level 3 Vest","game-helmet01":"Level 1 Helmet","game-helmet02":"Level 2 Helmet","game-helmet03":"Level 3 Helmet","game-1xscope":"1x Scope","game-2xscope":"2x Scope","game-4xscope":"4x Scope","game-8xscope":"8x Scope","game-15xscope":"15x Scope","game-level-1":"Lvl. 1","game-level-2":"Lvl. 2","game-level-3":"Lvl. 3","game-outfitBase":"Basic Outfit","game-outfitRoyalFortune":"Royal Fortune","game-outfitKeyLime":"Key Lime","game-outfitCobaltShell":"Cobalt Shell","game-outfitCarbonFiber":"Carbon Fiber","game-outfitDarkGloves":"The Professional","game-outfitGhillie":"Ghillie Suit","game-outfitCamo":"Forest Camo","game-outfitRed":"Target Practice","game-outfitWhite":"Arctic Avenger","game-outfitWoodland":"Woodland Combat","game-outfitJester":"Jester's Folly","game-outfitPrisoner":"The New Black","game-outfitCasanova":"Casanova Silks","game-outfitKhaki":"The Initiative","game-fists":"Fists","game-ak47":"AK-47","game-scar":"SCAR-H","game-dp28":"DP-28","game-mosin":"Mosin Nagant","game-m39":"M39 EMR","game-mp5":"MP5","game-mac10":"MAC-10","game-ump9":"UMP9","game-vector":"Vector","game-m870":"M870","game-mp220":"MP220","game-saiga":"Saiga-12","game-m9":"M9","game-m9_dual":"Dual M9","game-glock":"G18C","game-glock_dual":"Dual G18C","game-ot38":"OT-38","game-ot38_dual":"Dual OT-38","game-deagle":"DEagle 50","game-deagle_dual":"Dual DEagle 50","game-famas":"FAMAS","game-hk416":"M416","game-mk12":"Mk 12 SPR","game-m249":"M249","game-frag":"Frag Grenade","game-smoke":"Smoke Grenade","game-barrel_01":"a barrel","game-silo_01":"a silo","game-oven_01":"an oven","game-control_panel_01":"Control Panel","game-control_panel_02":"Control Panel","game-control_panel_03":"a computer terminal","game-power_box_01":"a power box"},Ml={language:kl};class Ll{slotIdToPlacement={survevio_728x90_leaderboard_top:"survevio_728x90_leaderboard",survevio_300x250_leaderboard_top:"survevio_300x250_leaderboard",survevio_300x250_leaderboard_bottom:"survevio_300x250_leaderboard",survevio_728x90_playerprofile_top:"survevio_728x90_playerprofile",survevio_300x250_playerprofile_top:"survevio_300x250_playerprofile",survevio_300x250_playerprofile_bottom:"survevio_300x250_playerprofile"};showFreestarAds(n){}getFreestarSlotPlacement(n){}}class Rl{el=L("#content");mainView;playerView;config;localization;view;adManager;constructor(){this.mainView=new bl(this),this.playerView=new Dl(this),L("#search-players").on("submit",n=>{n.preventDefault();const s=L("#search-players :input").val(),d=_l(s);window.location.href=`/stats/?slug=${d}`});try{const n=JSON.parse(localStorage.getItem("survev_config"));n.profile&&n.profile.slug&&L("#my-profile").css("display","block").attr("href",`/stats/?slug=${n.profile.slug}`)}catch{}this.config=new As,this.config.load(()=>{}),this.localization=new Ns("en",["en","es"],{en:Il},!0),this.localization.setLocale(this.config.get("language")),this.localization.localizeIndex(),this.adManager=new Ll,window.addEventListener("load",()=>{W.getParameterByName("slug")?this.setView("player"):this.setView("main")})}setView(n){n=="player"?this.view=this.playerView:this.view=this.mainView,this.view.load(),this.el.html(this.view.el),this.render()}render(){L("#language-select").html(Ml.language({code:this.localization.getLocale()})),L(".dropdown-language").off("click"),L(".dropdown-language").on("click",n=>{const s=n.target,d=L(s).attr("value");d&&(L("#selected-language").text(d.toUpperCase()),this.localization.setLocale(d),this.localization.localizeIndex(),this.config.set("language",d))})}}new Rl;
